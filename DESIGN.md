# DESIGN

> Primary deliverable. Target ~800–1500 words. Every major choice is written as
> **Decision / Why / Alternative / Why not / Trade-off**.
>
> **Status:** skeleton (Commit 1). Sections are filled in as the matching commit
> lands; the full write-up is Commit 12.

## 1. Data model

<!-- TODO (Commit 2/12): Mermaid ER diagram. Per table: shape, PK strategy
(UUID v7, app-side), each index tied to the query it serves, why this shape over
alternatives, what changes at 100x (partition + retention on the webhook tables;
deliveries → a real queue; read replicas for list endpoints). -->

## 2. Invoice state machine

<!-- TODO (Commit 6/12): diagram of states + transitions + trigger for each +
terminal states. Then: which transitions are reversible (none), and how invalid
transitions are rejected (conditional `UPDATE ... WHERE state = ANY(expected)`). -->

## 3. Payment correctness & failure modes

<!-- TODO (Commit 8/12): the four-mechanism table, then answers to (a)–(e)
structured explicitly, not as prose:
(a) two clients, same invoice, same instant
(b) tok_timeout (30s) — what the endpoint returns, what state things are left in,
    how the caller learns the eventual result
(c) PSP succeeded but the service crashed before persisting — does the customer
    get charged twice
(d) same idempotency key, different body
(e) POST /pay on an already-paid invoice
Name the concurrency mechanism (row FOR UPDATE for claim serialisation + partial
unique index for the one-pending invariant + conditional state update + PSP
idempotency) and why each alternative alone is insufficient. -->

## 4. Webhook design

<!-- TODO (Commit 9/12): signing scheme (HMAC-SHA256 over "<ts>.<body>"),
two-part replay protection (timestamp freshness + event-id dedupe), backoff with
concrete numbers (1m, 5m, 30m, 2h, 6h; 6 attempts; ~8h46m budget), what happens
to exhausted deliveries, the two reconciliation endpoints, and why/how delivery
is decoupled from the request path (outbox insert in the state-change tx; worker
never on the request path; no lock held during the POST). -->

## 5. API key model

<!-- TODO (Commit 4/12): `dodo_<key_id>_<secret>` format; storage (key_id
plaintext + unique, sha256(secret) as bytea); SHA-256 not Argon2 with the
entropy argument; transmission (Authorization: Bearer, TLS at the edge);
rotation; revocation (revoked_at, soft); blast radius if leaked + mitigations.
Contrast webhook-secret storage (plaintext — must recompute HMAC — needs KMS in
prod). -->

## 6. What I cut and why

<!-- TODO (Commit 12): 3–5 items, each What / Why omitted / What production needs.
Draft: draft invoices + editing; refunds & partial payments; production rate
limiting; broker-backed webhook queue + dunning; audit log + admin tooling;
full SSRF hardening. -->

## 7. Production readiness gap

<!-- TODO (Commit 12): top 3 — observability (metrics, OTLP traces, alerting on
stuck-pending count and webhook exhaustion rate); rate limiting + abuse controls;
audit log + admin tooling. Honourable mention: full SSRF hardening, refunds. -->

---

## Build log

_A running record of what each commit shipped and why, written as it happened.
It feeds the sections above; **delete this whole section before submission.**_

Each entry has the same shape: **What shipped**, **Design choices** (with the
reason, kept short), and **Verified** (how it was checked)._

---

### Commit 1 — scaffold  (`990130b`)

**What shipped**
Two-crate Cargo workspace (`invoice-service` as lib + bin, `mock-psp`), doc
skeletons, and the `Cents` money type.

**Design choices**
- *No `shared` crate.* The only types both binaries touch are a small PSP
  request/response pair; a crate holding that would be structure for its own sake.
- *`Cents(i64)` with a tiny surface* — `checked_add`, `checked_mul_qty(u32)`,
  `try_sum`. No division, no float conversion, no dollar formatting. Overflowing
  arithmetic returns `None` so the caller can reject it. `try_sum` is what the
  money path uses; a separate saturating `impl Sum` is for tests and logging only.
- *Dependency versions pinned once* in the workspace table; each crate enables a
  dependency only in the commit that first uses it, so no commit carries dead deps.

---

### Commit 2 — database schema  (`da429b7`)

**What shipped**
One migration (`0001_init.sql`) with all nine tables and their indexes.

**Design choices**
- *Migrations run at app startup.* One service, one writer — a separate migrate
  step is a production concern (see section 7), not something to build now.
- *State columns are `TEXT` + a `CHECK` constraint, not a Postgres `ENUM`.* A
  CHECK is a one-line migration to change; `ENUM` value additions and their
  ordering are a footgun.
- *Cross-tenant integrity lives in the schema.* `customers` has
  `UNIQUE (id, business_id)`, and `invoices` carries a composite foreign key
  `(customer_id, business_id)` — so an invoice pointing at another tenant's
  customer cannot be inserted, regardless of what the query says.
- *`one_pending_payment_per_invoice`* — a partial unique index
  (`... WHERE status = 'pending'`). This is the load-bearing concurrency
  invariant: at most one in-flight external charge per invoice, even across
  different idempotency keys.
- *`payment_attempts UNIQUE (business_id, idempotency_key)`* — the same client
  operation is processed once; retries replay.
- *Webhook tables are split.* `webhook_events` holds each payload once;
  `webhook_deliveries` is one row per (event, endpoint) and carries `lease_until`
  for the claim/lease delivery worker.
- Every index is written against a concrete query (list customers, list invoices
  by state, poll due deliveries, replay the event log).
- *At 100×:* the webhook tables are the write-heavy ones → time-based
  partitioning plus a retention job, then a real queue for delivery.

**Verified** (throwaway PG18 cluster)
All tables and indexes create. The composite FK rejects an invoice that
references another tenant's customer. The partial unique index rejects a second
`pending` payment attempt on the same invoice.

**Correction**
`rust-toolchain.toml` (added in Commit 1) pinned `1.98.0`, which resolved to the
MSVC host toolchain on this machine while the working one is GNU — the build
failed at the linker. Removed it; the Rust version is now documented in the
README and will be pinned for Docker via the `rust:1.98` image.

---

### Commit 3 — config, error model, health, bootstrap  (`5df50a8`)

**What shipped**
`Config::from_env()`, the `ApiError` type, `/healthz` and `/readyz`, the
request-id middleware, migrations-on-startup, and graceful shutdown.

**Design choices**
- *Config is hand-rolled* (~60 lines): a typed struct, per-field parsing, and one
  error that names the offending variable. No config framework earns its place.
- *One `ApiError` enum → one JSON shape* `{"error":{"code","message","details"?}}`.
  `Internal` logs the real cause and returns an opaque body. Validation failures
  are `422` (the request parsed, it is semantically rejected), not `400`.
- *Liveness ≠ readiness.* `/healthz` never touches the database, so a slow DB
  can't get a healthy process killed. `/readyz` runs `SELECT 1` and returns `503`
  while the DB is unreachable.
- *Request id:* reuse an incoming `x-request-id` or mint a UUID v7; attach it to
  the tracing span and echo it on the response. It never appears in a response
  body.
- *Migrations run via `sqlx::migrate!()` on startup;* shutdown drains on Ctrl-C /
  SIGTERM.

**Deviation from the plan**
The plan wanted compile-time-checked queries (`sqlx::query!`) from the start.
Those need a database (or a committed `.sqlx` cache) at *build* time, which would
also make the unit tests need a database. Chose unchecked `sqlx::query` instead —
every `cargo build` / `test` stays DB-free and the Docker image needs no `.sqlx`
bundle. The SQL is simple and gets exercised by the integration tests in
Commit 10.

**Verified**
Migrations run on startup. With Postgres stopped, `/healthz` stays `200` and
`/readyz` returns `503`.

---

### Commit 4 — API key authentication  (`6e2795c`)

**What shipped**
Token generation, the `require_api_key` middleware, the `Business` extractor, and
the `invoice-service seed` subcommand.

**Design choices**
- *Token is `dodo_<key_id>_<secret>`.* `key_id` is stored in plaintext and
  uniquely indexed, so authentication is a single-row lookup with no prefix-scan
  ambiguity. Only `sha256(secret)` is stored.
- *SHA-256, not Argon2/bcrypt.* The secret is 256 bits of CSPRNG output, so a
  slow KDF only adds latency to every request and defends against low-entropy
  guessing that cannot happen here.
- *Constant-time comparison* (`subtle`) of the hashes — a timing leak would
  reveal only bits of `sha256(guess)`, but the check is free.
- *Revocation is a `revoked_at` timestamp* (soft), so an audit trail survives;
  the lookup treats a non-null value as revoked.

**Deviation from the plan**
The plan specified base62 key material. Used hex — no big-integer encoder, no
extra dependency, and hex never contains `_` so splitting the token needs no
escaping. Same entropy (96-bit id, 256-bit secret).

**Verified**
`invoice-service seed` against a real Postgres: two runs create two businesses,
each `secret_hash` is 32 bytes, `revoked_at` is null.

---

### Commit 5 — customers

**What shipped**
`POST /v1/customers`, `GET /v1/customers/{id}`, `GET /v1/customers` (paginated),
and the `/v1` route group placed behind `require_api_key`.

**Design choices**
- *Tenant scoping is in every query.* Each statement carries
  `WHERE business_id = $1`; a `GET` for another tenant's customer is a plain
  `404`, not a `403` (we don't confirm the row exists).
- *Keyset pagination, not `OFFSET`.* Order is `(created_at DESC, id DESC)`, and a
  page is "everything strictly less than the last row's `(created_at, id)`". This
  is O(rows returned), stable under inserts, and matches `customers_list_idx`
  exactly. `OFFSET` drifts when rows are added and scans everything it skips.
- *The cursor is opaque* — base64 of `<nanos>_<uuid>`. Clients round-trip it and
  don't build their own.
- *Response envelopes are consistent:* lists return
  `{ "data": [...], "next_cursor": ... }`; a single resource returns the bare
  object.
- *Email validation is deliberately loose* — one `@`, non-empty local part, a dot
  in the domain. Not RFC 5322; just enough to reject typos.

**Verified** (curl, against the dev cluster)
No key → `401`; bad key → `401`. Create → `201` with the row. Bad body → `422`
with the standard error envelope and per-field messages. `GET` by id → `200`;
unknown id → `404`. `?limit=2` returns two newest plus a `next_cursor`; passing
that cursor returns the next two with `next_cursor: null` at the end — no overlap,
correct order.

---

### Commit 6 — invoices and the state machine

**What shipped**
`POST /v1/invoices` (server computes the total), `GET /v1/invoices/{id}` (with
line items), `GET /v1/invoices?state=` (paginated), `POST .../void`,
`POST .../mark-uncollectible`. Plus `invoice_state.rs` (the machine +
`transition_invoice`) and `outbox.rs` (the transactional outbox).

**Design choices**
- *The server owns the total.* Each line amount is `unit_amount_cents ×
  quantity` via `Cents::checked_mul_qty`; the total is `Cents::try_sum`. Any
  overflow is a `422`, never a wrap or panic.
- *A client-supplied `total` (or any unknown field) is rejected*, not ignored —
  `#[serde(deny_unknown_fields)]`. Being loud beats silently dropping it.
- *State transitions are one conditional `UPDATE`*:
  `SET state = $to WHERE id = ? AND business_id = ? AND state = ANY($from)`. Zero
  rows updated → re-read to return `404` (no such invoice) vs `409`
  (`invalid_state_transition` with `from`/`to`). No trigger, no read-then-write,
  no `SERIALIZABLE`.
- *`open` is the only entry point and the only non-terminal state.* No
  transition is reversible. The whole table is also a pure function
  (`InvoiceState::can_transition_to`) with an exhaustive unit test.
- *The `invoice.created` event is written in the same transaction as the insert.*
  If the insert rolls back the event goes with it — no orphan webhook. Fan-out to
  `webhook_deliveries` is one row per active endpoint (zero for now; endpoint
  registration comes later).
- *Line items are immutable* — no PATCH endpoint. (A deliberate cut; see
  section 6.)

**Rough edge**
`deny_unknown_fields` and bad JSON are rejected by axum's own `Json` extractor,
so those responses are `422` with axum's plain-text body rather than the
`{"error":{…}}` envelope. Everything the handlers reject themselves uses the
envelope. A custom `Json` extractor would unify it; not worth it here.

**Verified** (curl, against the dev cluster)
Total is computed server-side (`2×1500 + 3×99 = 3297`), per-line amounts filled
in. Client `total_cents` → `422`. Empty `line_items`, `quantity < 1`, negative
amount → `422` with per-field messages. `GET` returns the invoice with its lines.
`?state=open` lists it, `?state=paid` is empty. `void` → `200` state `void`;
voiding again → `409 invalid_state_transition from void to void`;
`mark-uncollectible` on a void invoice → `409`. A `webhook_events` row for
`invoice.created` exists with the invoice as `resource_id`.

---

### Dev tooling — local Postgres helper  (`99f546f`)

Not part of the plan. `scripts/pg-dev.sh` runs a throwaway Postgres in
`./.pgdata` on port 5433, isolated from any system install, with fixed
credentials (`dodo`/`dodo`). Added so the service can be run and checked by hand
without Docker.
