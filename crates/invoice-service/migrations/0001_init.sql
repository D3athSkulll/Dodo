-- Full schema in one migration. Single service, single writer, so migrations
-- run at app startup (Commit 3). Targets Postgres 16+ (no version-specific SQL).

-- The tenant. Everything else hangs off a business.
create table businesses (
    id         uuid primary key,
    name       text not null,
    created_at timestamptz not null default now()
);

-- API keys scoped to a business. Token format is `dodo_<key_id>_<secret>`.
-- key_id is plaintext + unique, so auth is one indexed lookup, no prefix scan.
-- Only sha256(secret) is stored.
create table api_keys (
    id          uuid primary key,
    business_id uuid not null references businesses,
    key_id      text  not null unique,
    secret_hash bytea not null,
    name        text,
    created_at  timestamptz not null default now(),
    revoked_at  timestamptz              -- soft revoke; keeps an audit trail
);

-- A customer belongs to exactly one business.
-- UNIQUE (id, business_id) looks redundant next to the PK, but it lets invoices
-- carry a composite FK (below) so an invoice can never reference another
-- tenant's customer.
create table customers (
    id          uuid primary key,
    business_id uuid not null references businesses,
    name        text not null,
    email       text not null,
    created_at  timestamptz not null default now(),
    unique (id, business_id)
);

-- List customers newest-first; keyset pagination on (created_at, id).
create index customers_list_idx on customers (business_id, created_at desc, id desc);

-- Invoices. total_cents is server-computed; currency is USD-only for now.
create table invoices (
    id          uuid primary key,
    business_id uuid not null references businesses,
    customer_id uuid not null,
    state       text   not null default 'open'
                check (state in ('open', 'paid', 'void', 'uncollectible')),
    total_cents bigint not null check (total_cents >= 0),
    currency    text   not null default 'USD' check (currency = 'USD'),
    due_date    date   not null,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now(),
    -- composite FK: (customer_id, business_id) must match a real customer row,
    -- so a cross-tenant invoice is unrepresentable, not just filtered out.
    foreign key (customer_id, business_id) references customers (id, business_id)
);

-- List invoices filtered by state, newest-first, keyset-paginated.
create index invoices_list_idx on invoices (business_id, state, created_at desc, id desc);

-- Line items are immutable after creation (no PATCH endpoint).
-- amount_cents = unit_amount_cents * quantity, computed by the app with checked
-- arithmetic; stored so totals never need recomputing on read.
create table invoice_line_items (
    id                uuid primary key,
    invoice_id        uuid not null references invoices on delete cascade,
    description       text   not null,
    quantity          int    not null check (quantity > 0),
    unit_amount_cents bigint not null check (unit_amount_cents >= 0),
    amount_cents      bigint not null check (amount_cents >= 0)
);

-- One row per attempt at paying an invoice.
create table payment_attempts (
    id                  uuid primary key,
    invoice_id          uuid not null references invoices,
    business_id         uuid not null,
    idempotency_key     text  not null,
    request_fingerprint bytea not null,   -- sha256(invoice_id || card_token)
    status              text  not null check (status in ('pending', 'succeeded', 'failed')),
    psp_ref             text,
    failure_code        text,
    amount_cents        bigint not null,
    last_error          text,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    -- same client operation (same key) is processed once; retries replay it.
    unique (business_id, idempotency_key)
);

-- The load-bearing concurrency invariant: at most one in-flight external charge
-- per invoice, even across different idempotency keys. A second concurrent /pay
-- collides here and gets 409 payment_in_progress.
create unique index one_pending_payment_per_invoice
    on payment_attempts (invoice_id) where status = 'pending';

-- Endpoints a business registers. secret is plaintext because the worker has to
-- recompute the HMAC on every send.
create table webhook_endpoints (
    id          uuid primary key,
    business_id uuid not null references businesses,
    url         text not null,
    secret      text not null,
    active      boolean not null default true,
    created_at  timestamptz not null default now()
);

-- The event: payload stored once. Also the durable log a business replays from
-- to reconcile missed webhooks.
create table webhook_events (
    id          uuid primary key,
    business_id uuid not null references businesses,
    event_type  text not null,
    resource_id uuid not null,
    payload     jsonb not null,
    created_at  timestamptz not null default now()
);

create index webhook_events_log_idx on webhook_events (business_id, created_at desc, id desc);

-- The attempt: one row per (event, endpoint). No payload here — it joins to
-- webhook_events. lease_until lets the worker claim a row, commit, POST outside
-- any transaction, then record the outcome; a crashed worker's rows free
-- themselves once the lease expires.
create table webhook_deliveries (
    id              uuid primary key,
    event_id        uuid not null references webhook_events,
    endpoint_id     uuid not null references webhook_endpoints,
    status          text not null
                    check (status in ('pending', 'inflight', 'delivered', 'exhausted')),
    attempts        int  not null default 0,
    next_attempt_at timestamptz not null default now(),
    lease_until     timestamptz,
    last_error      text,
    created_at      timestamptz not null default now(),
    delivered_at    timestamptz
);

-- The worker polls for due work: pending/inflight rows ordered by next_attempt_at.
create index deliveries_due_idx on webhook_deliveries (next_attempt_at)
    where status in ('pending', 'inflight');
