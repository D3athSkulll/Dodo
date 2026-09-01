# Build Prompt — Invoice & Payment Service (Dodo Payments Backend Take-Home)

> This is the **authoritative build instruction**. Read it top to bottom before writing
> any code. It folds together the assignment spec, a reviewed design, and the commit plan.
>
> **Guiding principle:** every mechanism here exists because there is one specific
> invariant it protects, and that invariant can be explained out loud, unscripted, on
> camera. Payment correctness, concurrency, and failure semantics are the centre of
> gravity. Restraint is graded — anything not required is written up in `DESIGN.md`,
> not built.

---

## 0. Operating rules for the implementer

1. **Follow the commit plan in §11.** Each commit compiles, is a coherent milestone, and
   its body explains *why* plus a `Design decisions to review:` block.
2. **Author is the candidate only.** No AI co-author trailer on assignment commits.
   ```bash
   git config user.name  "D3athSkulll"
   git config user.email "shivam.deolankar@gmail.com"
   ```
3. **If, while building, an alternative is simpler, safer, or more honest for a 4–6h
   take-home, take it** — and record the change in the commit body and `DESIGN.md`.
   Do not preserve a decision just because it is written here.
4. **The bar is not sophistication.** It is: a small, correct service where every
   mechanism maps to one named invariant the candidate can explain without notes.
5. **Time budget is 4–6 focused hours.** If time runs short, degrade in the order in
   §16 and document what was stubbed.
6. Never put `f32`/`f64`/`Decimal` anywhere in the money path. Integer minor units only.

---

## 1. What is being built and how it is graded

Build a minimal **Invoice & Payment Service** — the backend of a billing product. A
business authenticates with API keys, creates customers and invoices, customers pay
invoices through a **mock PSP**, and the business is notified of state changes via
**signed webhooks**.

The product surface is intentionally small. The interesting work is the **state
machine, failure modes, and data model.**

### Grading axes (weighted roughly equally)

| Axis | What they look for |
|---|---|
| **Design judgment** (`DESIGN.md`, video) | Specific reasoning. State machine coherent and complete. Failure-mode answers specific, not generic. Can explain the design verbally without notes. |
| **Core correctness** | Money math is integer. Concurrent payments do not double-charge. Idempotency works. Invoice transitions valid. PSP failures never corrupt state. |
| **Operational sense** | Migrations, one consistent error format, sensible logging, clean `docker compose up`, webhooks decoupled from the request path. |
| **Communication** (`AI_USAGE.md`, `README`, commits, video) | Honest, specific, shows what the candidate contributed. Video walkthrough is fluent and unscripted. |

**Does NOT score points:** lines of code, features beyond the must-haves, exhaustive
test coverage, fancy abstractions, premature optimization, unjustifiable dependencies.

### Required deliverables (a GitHub repo)

- Source code — invoice service **plus** mock PSP (same repo).
- Migrations — SQL files.
- `docker-compose.yml` — one-command setup, no manual steps.
- `README.md` — run instructions, 3–4 curl examples (create customer, create invoice,
  successful payment, failing payment), and a **Demo Video** section with the link.
- `DESIGN.md` — **the primary deliverable**, ~800–1500 words, sections in the spec's
  order (see §15).
- `AI_USAGE.md` — honest, specific (see §15).
- API documentation — OpenAPI YAML or Markdown, with request/response shapes and one
  consistent error format.
- **Demo video**, 5–10 min, accessible without login, link in `README.md` (see §15).

---

## 2. Scope guardrails

### Must-have (build these)

1. API key authentication — scoped to a business; storage, hashing, transmission,
   revocation all defended in `DESIGN.md`.
2. Customers — create, get, list (scoped to the authenticated business).
3. Invoices — create with line items `{description, quantity, unit_amount_cents}`;
   **server computes the total, never trusts a client total**; get by id; list
   filterable by state.
4. Payment attempts — `POST /v1/invoices/{id}/pay` with a mock card token; records an
   attempt, calls the mock PSP, updates invoice state from the result; **idempotent via
   `Idempotency-Key` header**; handles a slow or failing PSP without corrupting invoice
   state.
5. Invoice state machine — states defined below; a diagram in `DESIGN.md` with every
   transition, its trigger, and which states are terminal; invalid transitions rejected
   at the API with a clear error.
6. Webhooks — businesses register endpoint URLs; receive **signed** `invoice.created`,
   `invoice.paid`, `invoice.payment_failed`; retried on failure with a **documented
   backoff**; delivery **must not block the API response**.
7. PostgreSQL with migrations.
8. `docker compose up` brings up app + DB + mock PSP with no further steps.
9. `README` with run instructions and curl examples.
10. API documentation with a consistent error format.

### Explicitly out of scope (do NOT build; mention in `DESIGN.md` if relevant)

Subscriptions / recurring billing / plans / proration · refunds & partial payments ·
multi-currency / FX · tax calculation · frontend / UI · real email sending
(`tracing::info!("would send email …")` is fine) · production-grade rate limiting ·
OAuth or any auth beyond API keys.

> If tempted to build something not on the must-have list, write about it in
> `DESIGN.md` §6 instead. What was cut, and why, is a graded section.

---

## 3. Stack & dependencies

| Area | Choice | One-line rationale |
|---|---|---|
| Language / framework | Rust stable + **Axum 0.8** + Tokio | Spec preference; typed extractors, Tower ecosystem. |
| DB | PostgreSQL 16 | Required. |
| DB access | **SQLx 0.8** — async, compile-time-checked queries, built-in migrator | Raw SQL keeps money math, conditional `UPDATE`s, and partial unique indexes legible. No ORM. |
| Money | `i64` minor units; thin `Cents(i64)` newtype. **No float / `Decimal` in the money path.** | Spec calls this out. Newtype blocks mixing with counts. |
| IDs | **UUID v7**, generated app-side | Unique without coordination; time-ordered → index locality; sortable in logs. Not an enumeration defence. |
| Auth | API key `dodo_<key_id>_<secret>`; store `key_id` (plaintext, unique) + `sha256(secret)` | High-entropy secret → SHA-256 sufficient; `key_id` gives O(1) exact lookup. |
| Webhook signing | HMAC-SHA256 over `"<timestamp>.<raw_body>"`, header `Dodo-Signature: t=<ts>,v1=<hex>` | Symmetric per-endpoint secret; timestamp + `event_id` give replay protection. |
| Async webhook delivery | Postgres **outbox** (`webhook_events` + `webhook_deliveries`) + in-process Tokio worker, **claim/lease** pattern | No broker → one-command setup; survives restart; never on the request path. |
| PSP client | `reqwest`, **hard 5s timeout**, forwards `idempotency_key` | `tok_timeout` sleeps 30s; the endpoint must return fast and leave a recoverable state. |
| Mock PSP | Second workspace binary, own container, one route `POST /charge` (idempotent on `idempotency_key`) + `GET /_debug/charges` for tests | Real external dependency, minimal surface. |
| Config | hand-rolled `Config::from_env() -> Result<Config, ConfigError>` | ~20 lines; no framework earns its place here. |
| Errors | one `ApiError` enum → single JSON body `{"error":{"code","message","details"?}}` via `IntoResponse` | Consistent error format is graded. |
| Logging | `tracing` + `tracing-subscriber` (JSON) + `tower-http` `TraceLayer` + ~12-line `request_id` middleware | "Sensible logging" is graded; `request_id` stays in logs/traces only. |
| Tests | `sqlx::test` (isolated DB per test) + `reqwest` against a spawned app; `tokio::task::JoinSet` for concurrency | The three required tests on real Postgres, no fixture leakage. |
| Migrations | `sqlx::migrate!()` on app startup | Single service, single writer. Prod would separate; documented. |
| API docs | hand-written `openapi.yaml` + short `API.md` | No macro-doc drift. |

### Workspace layout

```
Cargo.toml                      # workspace: members = crates/invoice-service, crates/mock-psp
rust-toolchain.toml             # pin the exact stable used
crates/invoice-service/
  src/main.rs  src/config.rs  src/error.rs  src/money.rs  src/auth.rs
  src/state.rs  src/psp.rs  src/webhooks/…  src/routes/…  src/repo/…
  migrations/0001_init.sql
  tests/concurrency.rs  tests/idempotency.rs  tests/psp_failure.rs  tests/concurrent_timeout.rs
crates/mock-psp/src/main.rs
docker-compose.yml  Dockerfile  .env.example
README.md  DESIGN.md  AI_USAGE.md  API.md  openapi.yaml
```

No `shared` crate — the only cross-binary types are a 3-field PSP request/response pair;
duplicate them.

### Dependency list (locked in Commit 1 — each line justified)

```toml
axum = "0.8"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "time"] }   # not "full"
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres", "uuid", "time", "macros", "migrate"] }
uuid = { version = "1", features = ["v7"] }
time = { version = "0.3", features = ["serde", "macros"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
hmac = "0.12"
sha2 = "0.10"
getrandom = "0.2"            # CSPRNG bytes for key_id / secret / webhook secret
subtle = "2"                # constant-time hash compare — OPTIONAL (see §10); keep unless it bothers the reviewer
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tower-http = { version = "0.6", features = ["trace"] }
```

Dropped as deliberate restraint: `figment`/`envy` (hand-rolled config), `anyhow`
(`thiserror` + one `ApiError` enum is enough), `futures` (`JoinSet` covers the
concurrency test), `rand` (→ `getrandom`).

---

## 4. Architecture overview (say this in the video, 1–2 min)

```
client ──API key──> invoice-service (Axum :8080) ──HTTP 5s timeout──> mock-psp (:9090)
                          │
                          ├── PostgreSQL :5432  (invoices, payment_attempts, outbox …)
                          │
                          ├── payment reconciliation sweeper   (Tokio task, polls pending attempts)
                          └── webhook delivery worker          (Tokio task, claim/lease on the outbox)
```

Request flow for a payment: **claim** (short DB tx, no I/O) → **call PSP** (no tx open) →
**settle** (short DB tx). Webhook rows are inserted in the *same* tx as the state change;
a separate worker delivers them off the request path.

---

## 5. Data model & migrations (Commit 2)

Single migration `migrations/0001_init.sql`.

- `businesses(id uuid pk, name text not null, created_at timestamptz not null default now())`
- `api_keys(id uuid pk, business_id uuid not null references businesses, key_id text not null unique, secret_hash bytea not null, name text, created_at timestamptz not null default now(), revoked_at timestamptz null)`
- `customers(id uuid pk, business_id uuid not null references businesses, name text not null, email text not null, created_at timestamptz not null default now(), unique (id, business_id))`
  - `customers_list_idx on (business_id, created_at desc, id desc)`
- `invoices(id uuid pk, business_id uuid not null references businesses, customer_id uuid not null, state text not null default 'open' check (state in ('open','paid','void','uncollectible')), total_cents bigint not null check (total_cents >= 0), currency text not null default 'USD' check (currency = 'USD'), due_date date not null, created_at timestamptz not null default now(), updated_at timestamptz not null default now(), foreign key (customer_id, business_id) references customers (id, business_id))`
  - `invoices_list_idx on (business_id, state, created_at desc, id desc)`
- `invoice_line_items(id uuid pk, invoice_id uuid not null references invoices on delete cascade, description text not null, quantity int not null check (quantity > 0), unit_amount_cents bigint not null check (unit_amount_cents >= 0), amount_cents bigint not null check (amount_cents >= 0))`
- `payment_attempts(id uuid pk, invoice_id uuid not null references invoices, business_id uuid not null, idempotency_key text not null, request_fingerprint bytea not null, status text not null check (status in ('pending','succeeded','failed')), psp_ref text null, failure_code text null, amount_cents bigint not null, last_error text null, created_at timestamptz not null default now(), updated_at timestamptz not null default now())`
  - `unique (business_id, idempotency_key)` — **client-operation dedupe**
  - `create unique index one_pending_payment_per_invoice on payment_attempts (invoice_id) where status = 'pending'` — **at most one in-flight external charge per invoice**
- `webhook_endpoints(id uuid pk, business_id uuid not null references businesses, url text not null, secret text not null, active bool not null default true, created_at timestamptz not null default now())`
- `webhook_events(id uuid pk, business_id uuid not null references businesses, event_type text not null, resource_id uuid not null, payload jsonb not null, created_at timestamptz not null default now())`
  - `(business_id, created_at desc, id desc)` for the reconcile/replay endpoint
- `webhook_deliveries(id uuid pk, event_id uuid not null references webhook_events, endpoint_id uuid not null references webhook_endpoints, status text not null check (status in ('pending','inflight','delivered','exhausted')), attempts int not null default 0, next_attempt_at timestamptz not null default now(), lease_until timestamptz null, last_error text null, created_at timestamptz not null default now(), delivered_at timestamptz null)`
  - `create index deliveries_due_idx on webhook_deliveries (next_attempt_at) where status in ('pending','inflight')`

**Design points for DESIGN.md §1**

- `state`/`status` as `TEXT + CHECK`, not Postgres `ENUM`: `CHECK` is trivially altered
  later; `ENUM` value additions and ordering are footguns.
- Cross-tenant integrity **at the DB**: `customers UNIQUE (id, business_id)` +
  `invoices FK (customer_id, business_id)` makes "invoice pointing at another business's
  customer" unrepresentable — not just a `WHERE business_id` convention.
- Every index is written against a concrete query (list customers, list invoices by
  state, poll due deliveries, replay events): `WHERE` + `ORDER BY` + keyset tiebreaker.
- `one_pending_payment_per_invoice` — the load-bearing concurrency invariant.
- `total_cents` only, no `subtotal_cents` — no tax/discount/fee exists.
- UUID v7 app-side (vs `gen_random_uuid()`) keeps ID creation testable and lets us log
  the ID before the INSERT returns.
- At 100x: `webhook_deliveries` / `webhook_events` are write-heavy → time-based
  partitioning + retention job, eventually a real queue.

### `Cents` type (Commit 1) — build exactly this, nothing more

`Cents::new(i64)`, `Cents::ZERO`, `into_inner(self) -> i64`,
`checked_add(self, Cents) -> Option<Cents>`,
`checked_mul_qty(self, qty: u32) -> Option<Cents>` (line amount = unit × quantity),
`impl Sum for Cents` via `checked_add`.
**Deliberately absent:** `Div`, `Mul<f64>`, `From<f64>`, `as f64`, and any `Display`
that divides by 100 (formatting to dollars is presentation, not this type).

---

## 6. Invoice state machine (Commit 6)

```
            ┌─────────────────────────────┐
            │            open              │  ← created here (POST /v1/invoices)
            └──┬───────────┬───────────┬───┘
   payment ok  │           │ void      │ mark-uncollectible
               ▼           ▼           ▼
            ┌──────┐   ┌──────┐   ┌───────────────┐
            │ paid │   │ void │   │ uncollectible │
            └──────┘   └──────┘   └───────────────┘
             terminal    terminal      terminal
```

- `open` is the only non-terminal state and the only entry point. **No `draft`** — there
  is no draft-editing endpoint, so `draft` would be a state with no legal operation.
- Triggers: `open→paid` = a payment attempt succeeds (§7); `open→void` = `POST /void`;
  `open→uncollectible` = `POST /mark-uncollectible`.
- **No transition is reversible.** "Un-voiding" or "reopening a paid invoice" would be a
  new invoice, not a state change.
- Enforcement = **conditional `UPDATE`**, never a trigger, never read-then-write:
  ```
  transition_invoice(exec, id, business_id, expected: &[State], to: State):
    UPDATE invoices SET state = $to, updated_at = now()
     WHERE id = $1 AND business_id = $2 AND state = ANY($expected);
    -- rows_affected == 0  ->  re-read state  ->  ApiError::InvalidStateTransition { from, to }
  ```
- Why not a DB trigger (hidden control flow, hard to test), a Postgres `ENUM` (migration
  friction), or `SERIALIZABLE` (retry storms for a check one `WHERE` clause does
  atomically).
- `due_date` in the past is **accepted** — it just means already overdue. No `overdue`
  state (would need a scheduler; not required).
- Line items are immutable after creation (no PATCH) — a restraint cut, DESIGN.md §6.
- Overflow in totals is a `422`, not a panic or a silent wrap.

**Authoritative transition table (unit test in Commit 6, referenced by Commit 10)**

```
open           -> paid            allow
open           -> void            allow
open           -> uncollectible   allow
open           -> open            reject   (no-op is not a transition)
paid           -> {open,void,uncollectible}   reject
void           -> {open,paid,uncollectible}   reject
uncollectible  -> {open,paid,void}            reject
```

---

## 7. Payment correctness (Commit 8 — the core)

**No DB transaction ever wraps the PSP HTTP call.** Three phases.

### Endpoint

```
POST /v1/invoices/:id/pay { card_token }
Header: Idempotency-Key: <k>          (required -> 422 if missing)
```

### Phase 1 — claim (one short tx, no external I/O)

```
fingerprint = sha256(invoice_id ‖ card_token)          -- only payment-relevant fields
BEGIN;
  SELECT state FROM invoices WHERE id=$id AND business_id=$b FOR UPDATE;   -- brief, local only
  not found            -> ROLLBACK, 404
  state == 'paid'      -> ROLLBACK; if a succeeded attempt with key k exists, replay it (200);
                          else 409 invoice_not_open
  state != 'open'      -> ROLLBACK, 409 invoice_not_open { state }
  INSERT INTO payment_attempts (id, invoice_id, business_id, idempotency_key,
                                request_fingerprint, status='pending',
                                amount_cents = invoices.total_cents);
COMMIT;
```

Two unique constraints decide the `INSERT` outcome:

- violates `(business_id, idempotency_key)` → load the existing attempt:
  - fingerprint ≠ stored → `409 idempotency_key_conflict`
  - fingerprint = stored, `pending` → `202 { attempt_id }` + `Retry-After` (someone else
    is mid-flight for this exact op)
  - fingerprint = stored, `succeeded`/`failed` → replay that terminal result (`200`/`402`)
- violates `one_pending_payment_per_invoice` → `409 payment_in_progress` (a *different*
  key already has a pending charge for this invoice; caller retries later)

### Phase 2 — call the PSP (no tx open)

```
resp = http.post(PSP_BASE_URL/charge,
                 { card_token, amount_cents, idempotency_key: k }).timeout(5s)
```

### Phase 3 — settle (one short tx)

```
BEGIN;
  succeeded ->
    UPDATE payment_attempts SET status='succeeded', psp_ref=$ref WHERE id=$aid AND status='pending';
    transition_invoice(open -> paid);
    insert webhook_events 'invoice.paid' + one delivery row per active endpoint;
    COMMIT; return 200 { attempt: succeeded, invoice: paid }
  failed(code) ->
    UPDATE payment_attempts SET status='failed', failure_code=$code WHERE id=$aid AND status='pending';
    -- invoice stays 'open'
    insert webhook_events 'invoice.payment_failed' + deliveries;
    COMMIT; return 402 { attempt: failed, code }
  timeout | 5xx | connection error ->
    UPDATE payment_attempts SET last_error=$e, updated_at=now() WHERE id=$aid;   -- stays 'pending'
    COMMIT; return 202 { attempt_id, status: pending } + Retry-After: 5
```

### Reconciliation sweeper (Tokio task, every `PAYMENT_SWEEP_INTERVAL_MS`)

```
for each payment_attempts row  status='pending' AND updated_at < now() - 3s
        (claimed with FOR UPDATE SKIP LOCKED; no external I/O inside that tx):
   re-POST the same /charge with the same idempotency_key      -- PSP is idempotent
   run Phase 3 settle logic
   if still failing AND created_at < now() - PAYMENT_PENDING_MAX_AGE_SECONDS (300s):
       UPDATE status='failed', failure_code='psp_unreachable';
       emit invoice.payment_failed;  invoice stays 'open' (business retries with a new key)
```

### Read model

`GET /v1/invoices/:id/payments` and `GET /v1/payments/:id` — how a caller learns the
eventual result of a `202`.

### Four correctness mechanisms — each protects a distinct invariant (DESIGN.md §3 table)

| # | Mechanism | Invariant it protects | What it does NOT cover |
|---|---|---|---|
| 1 | `UNIQUE (business_id, idempotency_key)` | the *same client operation* is processed once; retries replay | two *different* keys for the same invoice |
| 2 | partial `UNIQUE (invoice_id) WHERE status='pending'` | **at most one in-flight external charge per invoice**, even across different keys | ordering of settled attempts |
| 3 | conditional `UPDATE … WHERE state='open'` | **at most one `open→paid`**; late winners no-op | in-flight duplication (that's #2) |
| 4 | PSP idempotency on `idempotency_key` | a retry after a transport-ambiguous first call does **not** create a second charge | anything the PSP itself can't dedupe |

### Answers to the five evaluator questions (verbatim in DESIGN.md §3)

- **(a) two clients, same instant, same invoice.** Both reach Phase 1. `FOR UPDATE`
  serialises the two `SELECT`s; whichever commits its `INSERT` first holds the only
  `pending` row (#2). The other's `INSERT` violates the partial unique index →
  `409 payment_in_progress`. Exactly one PSP call, exactly one possible `open→paid`
  (#3). No double charge.
- **(b) `tok_timeout` (30s).** The 5s client timeout fires in Phase 2. Phase 3 takes the
  timeout branch: attempt stays `pending`, invoice stays `open`, endpoint returns
  `202 { attempt_id }` + `Retry-After`. Caller polls `GET /v1/payments/:id` or waits for
  the `invoice.paid` webhook. The sweeper re-submits the idempotent charge; when the
  PSP's 30s elapses it returns `succeeded` with a `psp_ref`, the sweeper runs Phase 3 →
  invoice `paid`, `invoice.paid` emitted.
- **(c) PSP succeeded, service crashed before Phase 3.** The `pending` row was committed
  in Phase 1, *before* the PSP call. On restart the sweeper finds it, re-POSTs `/charge`
  with the same `idempotency_key`; the mock returns the *same* `psp_ref` (#4). Phase 3
  runs once → invoice `paid`. Customer charged exactly once.
- **(d) same key, different body.** `request_fingerprint` mismatches the stored one in
  Phase 1 → `409 idempotency_key_conflict`, no PSP call, no state change.
- **(e) `POST /pay` on a `paid` invoice.** Phase 1 reads `state='paid'` under the lock.
  Request's key matches the attempt that paid it → return that `200` (idempotent
  replay). Otherwise → `409 invoice_not_open`. Never a PSP call.

**Idempotency response model:** operation-based, not HTTP-replay. Same key → same
`payment_attempts` row; the *response evolves* (`202 pending` → later `200 succeeded` /
`402 failed`). Store `status`, `psp_ref`, `failure_code` — enough to render any later
response — **not** a frozen HTTP status/body blob. Document in `API.md` and `DESIGN.md`.

---

## 8. Mock PSP (Commit 7 — `crates/mock-psp`)

`POST /charge { card_token, amount_cents, idempotency_key }` → `{ status, psp_ref?, code? }`.

| token | behaviour |
|---|---|
| `tok_success` | sleep ~100ms → `{status:"succeeded", psp_ref:<uuid>}` |
| `tok_insufficient_funds` | sleep ~100ms → `{status:"failed", code:"insufficient_funds"}` |
| `tok_card_declined` | sleep ~100ms → `{status:"failed", code:"card_declined"}` |
| `tok_timeout` | sleep 30s → then `{status:"succeeded", psp_ref:<uuid>}` |
| `tok_network_error` | **always** respond `500` immediately (deterministic — no socket-drop, no alternation) |
| anything else | `422 {code:"unknown_token"}` |

- **Idempotent:** in-memory `HashMap<idempotency_key, StoredOutcome>`. A repeated key
  returns the identical outcome without re-running the delay or re-deciding.
  `tok_timeout` replays fast on the second call. `tok_network_error` stores nothing
  (500 is not an outcome) so it fails again.
- `GET /_debug/charges` → `[{idempotency_key, card_token, psp_ref, status}]` — tests
  assert "exactly one charge".
- In-memory map is **not durable across restarts** — acceptable for a mock; it is not
  pretending to be a production PSP.
- One route. Reconciliation re-submits `POST /charge`; deliberately no `GET /charge/:id`.
- Allow a shorter `tok_timeout` delay via env so `psp_failure.rs` stays fast.

---

## 9. Webhooks (Commit 9)

- `POST /v1/webhook_endpoints { url }` → `{ id, secret }` returned **once** (32 random
  bytes, base62). Minimal SSRF guard at registration: reject URLs resolving to
  loopback / private / link-local / `169.254.169.254` (best-effort, documented as
  incomplete).
- Outbox writes are wired in Commits 6 & 8: every domain state change writes **one**
  `webhook_events` row + **one `webhook_deliveries` row per active endpoint**, in the
  *same transaction* as the state change. Delivery rows carry no payload — they join to
  `webhook_events`.
- **Delivery worker** (Tokio task, every `WEBHOOK_WORKER_INTERVAL_MS`), claim/lease:
  ```
  BEGIN;
    SELECT d.*, e.payload, ep.url, ep.secret
      FROM webhook_deliveries d
      JOIN webhook_events e   ON e.id = d.event_id
      JOIN webhook_endpoints ep ON ep.id = d.endpoint_id
     WHERE d.status IN ('pending','inflight')
       AND d.next_attempt_at <= now()
       AND (d.lease_until IS NULL OR d.lease_until < now())
     ORDER BY d.next_attempt_at
     FOR UPDATE SKIP LOCKED
     LIMIT 50;
    UPDATE … SET status='inflight', lease_until = now() + interval '30s';
  COMMIT;                                  -- lock released here

  for each claimed row (NO tx open):
     ts  = now_unix
     sig = hex(hmac_sha256(secret, "{ts}.{body}"))
     resp = http.post(url, body,
              headers { "Dodo-Signature": "t={ts},v1={sig}", "Dodo-Event-Id": event_id }).timeout(5s)

  BEGIN;                                   -- second tx: record outcome
     2xx                              -> status='delivered', delivered_at=now()
     retryable (408/429/5xx/timeout/conn) -> attempts += 1;
         if attempts >= 6 -> status='exhausted', last_error=…
         else             -> status='pending', next_attempt_at = now() + backoff(attempts), last_error=…
     other 4xx (permanent)            -> status='exhausted', last_error='http {code}'
  COMMIT;
  ```
- **Backoff schedule** (attempt N → delay before next): `1m, 5m, 30m, 2h, 6h` after
  attempts 1..5; max **6 attempts**; total budget ≈ **8h 46m**. Jitter noted as a
  production improvement, not implemented.
- Crashed `inflight` rows are reclaimed once `lease_until < now()`.
- **Reconciliation for businesses:**
  - `GET /v1/webhook_events?after=<cursor>&limit=` — the durable event log; replay
    anything missed.
  - `GET /v1/webhook_deliveries?status=exhausted` — what never got through.
- Email: `tracing::info!("would send email …")` only.

**Design points for DESIGN.md §4**

- Signing: HMAC-SHA256 over `"{timestamp}.{raw_body}"`. Asymmetric (Ed25519) rejected —
  key distribution/rotation overhead with no threat model that needs it here.
- **Replay protection is two mechanisms:** (1) receiver rejects if `|now - t| > 300s`;
  (2) receiver dedupes on `Dodo-Event-Id`. Freshness alone permits replay inside the
  window — the event id closes it. Delivery is at-least-once by design; receivers must
  be idempotent.
- `webhook_events` vs `webhook_deliveries` split: the event (payload, once) is separate
  from each attempt (state, per endpoint). No payload duplication.
- **No lock during the HTTP POST.** Claim + lease in a tx, commit, POST, settle in a
  second tx. `SKIP LOCKED` lets multiple replicas share the queue without
  double-delivery of the same row.
- **Decoupled from the request path:** the API handler's tx only *inserts* delivery
  rows; it never makes an outbound call. `/pay` latency is independent of every
  registered endpoint's health.
- SSRF is a documented production gap (resolve-then-pin DNS, block metadata/private
  ranges at connect time, no-follow-redirects, egress proxy). Best-effort check
  included; the rest is DESIGN.md §7.

---

## 10. Auth (Commit 4)

- Token: `dodo_<key_id>_<secret>` — `key_id` = 16 chars base62 (12 random bytes),
  `secret` = 43 chars base62 (32 random bytes). `key_id` stored plaintext + `UNIQUE`;
  `secret_hash = sha256(secret)` stored as `bytea`.
- Generation helper returns the full token string exactly once.
- `auth` middleware: read `Authorization: Bearer dodo_<id>_<secret>` → split on `_` →
  look up by `key_id` (one row) → constant-time compare `sha256(secret)` to
  `secret_hash` → reject if `revoked_at IS NOT NULL` → put `BusinessId` in request
  extensions. `Business` extractor for handlers.
- `invoice-service seed` subcommand + a compose `seed` one-shot service that logs the
  key once.

**Design points for DESIGN.md §5**

- **key-id + secret, not a prefix scan.** `key_id` is unique → lookup is a single-row
  index hit; the secret is never a lookup term. No collision analysis needed.
- **SHA-256, not Argon2/bcrypt.** Password hashing slows brute force against
  *low-entropy* human input. A 256-bit random secret has ~10⁷⁷ keyspace; a KDF adds
  latency to every request and defends nothing. This is one of the three AI_USAGE
  "decided against the suggestion" entries.
- Constant-time compare: we compare *hashes*, not secrets, so a timing leak reveals
  only bits of `sha256(guess)` — low value. `subtle::ConstantTimeEq` kept as cheap
  insurance; acceptable to drop.
- Revocation = `revoked_at` timestamp (soft) → audit trail survives. Blast radius if
  leaked: full API access for that one business until revoked; scopes / IP allowlist /
  short-lived keys are DESIGN.md §5 / production.
- **Webhook secret is stored plaintext** — the service must recompute HMAC on every
  send, so it cannot be hashed. Production: envelope-encrypt with a KMS key.

---

## 11. Config, errors, health, bootstrap (Commit 3)

- `Config::from_env()` — typed struct, explicit per-field parse, actionable
  `ConfigError` messages.
- `ApiError` enum → `IntoResponse`:

  | variant | status | code |
  |---|---|---|
  | `Unauthorized` | 401 | `unauthorized` |
  | `NotFound` | 404 | `not_found` |
  | `Validation(Vec<FieldError>)` | 422 | `validation_error` |
  | `InvalidStateTransition { from, to }` | 409 | `invalid_state_transition` |
  | `IdempotencyKeyConflict` | 409 | `idempotency_key_conflict` |
  | `PaymentInProgress` | 409 | `payment_in_progress` |
  | `InvoiceNotOpen { state }` | 409 | `invoice_not_open` |
  | `PspUnavailable` | 502 | `psp_unavailable` |
  | `Internal` | 500 | `internal` (opaque body; detail only in logs) |

- Body shape everywhere: `{"error":{"code","message","details"?}}`.
- `AppState { pool, http: reqwest::Client, config }`.
- `tracing-subscriber` JSON; `TraceLayer`; `request_id` middleware (generate UUID v7,
  span field + response header `x-request-id`).
- `GET /healthz` → `200` always if the process is up (**no DB** — a slow DB should not
  get a healthy process killed).
- `GET /readyz` → `SELECT 1` + "migrations applied"; `503` if the DB is unreachable.
- `main` installs SIGTERM / ctrl-c → graceful shutdown (webhook worker + payment sweeper
  finish their current iteration).

### `.env.example` (committed, works as-is with compose defaults)

```
DATABASE_URL=postgres://dodo:dodo@db:5432/dodo
PSP_BASE_URL=http://mock-psp:9090
PSP_TIMEOUT_MS=5000
BIND_ADDR=0.0.0.0:8080
WEBHOOK_WORKER_INTERVAL_MS=1000
WEBHOOK_LEASE_SECONDS=30
PAYMENT_SWEEP_INTERVAL_MS=2000
PAYMENT_PENDING_MAX_AGE_SECONDS=300
RUST_LOG=info,invoice_service=debug
```

---

## 12. Commit plan (13 commits, each compiles, each a real milestone)

Message format: `<type>: <imperative summary>`, body explains *why*, then a
`Design decisions to review:` block.

1. `chore: scaffold workspace, toolchain, doc skeletons` — workspace `Cargo.toml`
   (two members, no `shared`), `rust-toolchain.toml`, `.rustfmt.toml`, `clippy.toml`,
   `.gitignore`, `.dockerignore`, `.env.example`; `README`/`DESIGN`/`AI_USAGE`
   skeletons with the spec's headings; both `main.rs` parse config, init tracing, log
   "listening", exit clean; `Cents` in `money.rs`.
2. `feat: schema and migrations` — `migrations/0001_init.sql` (§5).
3. `feat: config, error model, bootstrap, health` (§11).
4. `feat: API key authentication` (§10).
5. `feat: customers` — `POST /v1/customers`, `GET /v1/customers/:id` (404 unless
   `business_id` matches), `GET /v1/customers?limit=&cursor=` (**keyset** on
   `(created_at desc, id desc)`, opaque base64 cursor). Repo module; every query has
   `WHERE business_id = $1`. Minimal email sanity check (`@` + a dot after it). List
   envelope `{ "data": [...], "next_cursor": ... }`; bare object for single-resource GET.
6. `feat: invoices with server-computed totals + state machine` (§6) — `POST /v1/invoices`
   (reject empty `line_items`, `quantity <= 0`, `unit_amount_cents < 0`, line count
   > 500; per line `amount_cents = Cents(unit).checked_mul_qty(qty)?` → `422` on
   overflow; `total_cents = Σ` via `Cents::sum` → `422` on overflow; **reject a
   client-supplied total loudly**; one tx: insert invoice `open` + line items +
   `invoice.created` event + deliveries); `GET /v1/invoices/:id` (+ line items + state);
   `GET /v1/invoices?state=&limit=&cursor=` (keyset); `POST /v1/invoices/:id/void`;
   `POST /v1/invoices/:id/mark-uncollectible`; `transition_invoice` helper; exhaustive
   transition unit test.
7. `feat: mock PSP` (§8).
8. `feat: payment attempts — claim, call PSP, settle; + reconciliation sweeper` (§7).
9. `feat: webhooks — events, signed delivery, claim/lease worker, retries` (§9).
10. `test: concurrency, idempotency, PSP-failure (+ concurrent-timeout)` (§13).
11. `chore: dockerfiles and docker-compose` (§14).
12. `docs: DESIGN.md` (§15).
13. `docs: README, OpenAPI, AI_USAGE` (§15).

---

## 13. Tests (Commit 10 — `tests/`, each `sqlx::test` + spawned app + real mock PSP)

1. **`concurrency.rs`** (spec-required) — one `open` invoice; `JoinSet` fires **20**
   concurrent `POST /pay`, `tok_success`, 20 distinct idempotency keys. Assert:
   - exactly **one** `200 succeeded`; the other 19 are `409 payment_in_progress` (or a
     `202`/`200` replay if they raced the settle) — **none** is a second success;
   - `count(*) FROM payment_attempts WHERE invoice_id=$1 AND status='succeeded'` == **1**;
   - `GET /_debug/charges` shows **exactly one** charge for that invoice;
   - final invoice state == `paid`.
2. **`idempotency.rs`** (spec-required) — one `POST /pay` (`tok_success`), then the
   *same* key + same body again. Assert identical status + body; `/_debug/charges`
   count == **1**.
3. **`psp_failure.rs`** (spec-required) — two cases:
   - `tok_timeout`: endpoint returns in ~5–6s with `202`, attempt `pending`, invoice
     `open`. One sweeper tick after the (env-shortened) PSP delay → attempt `succeeded`,
     invoice `paid`, one charge.
   - `tok_network_error`: `202`, then after `PAYMENT_PENDING_MAX_AGE_SECONDS`
     (env-shortened to ~3s) + a sweeper tick → attempt `failed` `psp_unreachable`,
     invoice still `open` (retryable), never stuck `pending`.
4. **`concurrent_timeout.rs`** (not required, proves mechanism #2) — 20 concurrent
   `POST /pay`, 20 distinct keys, `tok_timeout`. Assert `/_debug/charges` shows
   **exactly one** in-flight charge, one `pending` attempt, invoice `open`.
5. **State-machine unit test** (from Commit 6) — the table in §6.

Tests shorten timeouts via env, not by mocking time. `README` notes that per-handler
tests are intentionally skipped and why (the three risk areas are covered; handlers are
thin).

---

## 14. Docker (Commit 11)

- Multi-stage `Dockerfile` (cargo-chef for dependency-layer caching) building both
  binaries; `rustls`, no OpenSSL.
- `cargo sqlx prepare` → committed `.sqlx/` offline data so the image builds with no
  live DB (keep it fresh).
- `docker-compose.yml`:
  - `db` — `postgres:16-alpine`, healthcheck `pg_isready`, one named volume.
  - `seed` — one-shot, same image, `command: seed`, `depends_on: { db: service_healthy }`,
    logs the API key once.
  - `mock-psp` — built from the workspace, `:9090`.
  - `app` — `:8080`, `depends_on: { db: service_healthy, mock-psp: service_started }`;
    on boot runs `sqlx::migrate!()` then serves.
  - All config via env from `.env` (defaults in `.env.example` work unchanged).
- Verify on a clean checkout: `docker compose up`, `curl /healthz`, run every README
  curl, observe webhook delivery logs.
- **Migrations on app startup**, not a separate `migrate` container: one service, one
  writer, simplest correct thing. Two replicas would race on boot — fine for
  single-instance; documented in DESIGN.md §7.

---

## 15. Documentation deliverables

### `DESIGN.md` (Commit 12 — the primary deliverable, ~800–1500 words)

Sections in the spec's order. Use **Decision / Why / Alternative / Why not / Trade-off**
for every major choice.

1. **Data model** — Mermaid ER diagram; per table: shape, PK strategy (UUID v7
   app-side), indexes tied to their query, why this shape, what changes at 100x
   (partition + retention on webhook tables; deliveries → a real queue; read replicas
   for list endpoints).
2. **State machine** — the §6 diagram; triggers; three terminal states; **no reversible
   transitions**; rejection via conditional `UPDATE`.
3. **Payment correctness & failure modes** — the four-mechanism table, then (a)–(e)
   from §7 **structured explicitly as (a)–(e)**, not buried in prose. Name the
   concurrency mechanism: row `FOR UPDATE` for claim serialization **plus** a partial
   unique index for the one-pending invariant **plus** conditional state update **plus**
   PSP idempotency — and why each alternative alone is insufficient (advisory locks:
   manual key lifecycle, leak on crash; `SERIALIZABLE`: retry storms; pure optimistic:
   still needs the guards).
4. **Webhook design** — signing scheme, two-part replay protection, backoff numbers +
   6-attempt / ~9h budget, exhausted-delivery handling, the two reconciliation
   endpoints, why delivery is decoupled and *how* (outbox insert in the state-change
   tx; worker never on the request path; no lock during POST).
5. **API key model** — `key_id`+`secret` format, SHA-256 (not Argon2) with the entropy
   argument, transmission (`Authorization: Bearer`, TLS assumed at the edge), rotation
   (issue new, revoke old), revocation (`revoked_at`), blast radius + mitigations.
   Webhook-secret storage contrasted (plaintext, needs KMS in prod).
6. **What I cut** — 3–5 items, each `What / Why omitted / What the production version
   needs` (table in §17).
7. **Production readiness gap** — top 3: (1) observability (metrics, OTLP traces,
   dashboards, alerting on stuck-pending count and webhook exhaustion rate); (2) rate
   limiting + abuse controls (token bucket per `key_id`; connection limits); (3) audit
   log + admin tooling (key rotation, manual invoice ops, forced webhook replay) —
   honourable mention: full SSRF hardening, refunds.

### `README.md` (Commit 13)

- One-command run (`docker compose up`); where the seeded API key appears (the `seed`
  service logs).
- **Curl walkthrough** (copy-paste, against `:8080`): create customer → create invoice →
  `GET` invoice → pay with `tok_success` (→ `200 paid`) → pay a second invoice with
  `tok_card_declined` (→ `402`) → `GET /v1/webhook_deliveries` to show fan-out →
  optional `tok_timeout` to show the `202` + sweeper.
- **Demo Video** section with the link (accessible without login).
- Test list + explicit note that per-handler tests are intentionally skipped.

### `openapi.yaml` + `API.md` (Commit 13)

Every endpoint's request/response shape, the one error envelope, the
idempotency-response-model note, the auth header.

### `AI_USAGE.md` (Commit 13 — graded; be specific, not generic)

1. **Which tools, where** — e.g. Claude for the first schema draft and this plan; editor
   autocomplete for handler/DTO boilerplate; Claude to talk through Postgres partial
   unique indexes.
2. **Three decisions made against / independent of AI:**
   - **Rejected holding a DB row lock across the PSP HTTP call** (AI's first cut proposed
     it "bounded by the timeout"). Chose the three-phase claim/call/settle with a
     partial unique index — a lock cannot prevent a second *external* charge once the
     PSP may have accepted the first, and it starves the connection pool for 5–30s.
   - **Rejected Argon2/bcrypt for API-key storage.** Chose SHA-256: the secret is 256
     bits of CSPRNG output, so a slow KDF adds per-request latency and defends nothing.
   - **Rejected a `shared` crate / extra abstraction layers.** Two crates, thin modules
     — the take-home rewards restraint and there was no real reuse.
3. **One thing AI got wrong / corrected** — fill in with what actually happens during
   the build (e.g. an early draft emitted `invoice.created` outside the creation
   transaction → moved the outbox write into the same tx; verified by a test that
   aborts the creation tx and asserts no orphan delivery row).

### Demo video (5–10 min, required, link in README, accessible without login)

Cover, **in this order**:
1. **Architecture overview (1–2 min)** — services, data model, how a request flows
   API → DB → webhook delivery.
2. **Live demo (2–3 min)** — run `docker compose up`; create a customer, create an
   invoice, a successful payment, a failing payment (`tok_card_declined`), show the
   resulting webhook deliveries.
3. **State machine walkthrough (1–2 min, unscripted)** — why these states, allowed
   transitions, what is terminal, where the judgment calls were.
4. **Failure-mode walkthrough (1–2 min, unscripted)** — pick ONE failure mode from
   DESIGN.md §3, open the relevant file on camera, walk the lines. Use `tok_timeout` or
   `tok_network_error` to demo it live if desired.

No editing. Cuts, ums, retries are fine — the working session, not a marketing reel.

---

## 16. Major design decisions — Decision / Why / Alternative / Why not / Trade-off

| Decision | Why | Alternative | Why not | Trade-off accepted |
|---|---|---|---|---|
| **SQLx (no ORM)** | Money math, conditional `UPDATE`s, partial unique indexes, `FOR UPDATE SKIP LOCKED` are clearer as SQL; compile-time query checks. | SeaORM / Diesel | Extra abstraction; obscures the SQL correctness depends on. | More row→struct mapping boilerplate. |
| **UUID v7, app-side** | Unique with no coordination; time-ordered → index locality; sortable in logs. | UUID v4 | Random → index page splits under insert load; no temporal signal when debugging. | v7 leaks rough creation time (acceptable). |
| **`total_cents` only** | No tax/discount/fee exists. | `subtotal` + `total` | Storing a distinction the domain doesn't have. | A migration the day tax appears. |
| **Invoice born `open`; no `draft`** | No draft-editing endpoint → `draft` has no legal operation. | `draft → open` via `/finalize` | Ceremony state. | Can't stage an invoice before it's "real" — out of scope anyway. |
| **Claim / call / settle in 3 phases; no tx around the PSP call** | A lock can't stop a second *external* charge; don't hold locks/connections across a 5–30s call. | `BEGIN; SELECT FOR UPDATE; call PSP; UPDATE; COMMIT` | Connection-pool starvation; false safety; still needs idempotency for case (c). | A short window where a `pending` attempt exists before the PSP call — the sweeper closes it. |
| **Partial `UNIQUE (invoice_id) WHERE status='pending'`** | Enforces "≤ 1 in-flight external charge per invoice" across *different* keys. | Rely on `FOR UPDATE` alone | `FOR UPDATE` serialises but still lets N sequential PSP calls through for N keys. | A legitimate second payer waits/retries while one attempt is pending. |
| **Conditional `UPDATE … WHERE state='open'`** | Atomic single-winner `open→paid`; no read-then-write race. | Trigger / `ENUM` / `SERIALIZABLE` | Hidden logic / migration friction / retry storms for what one `WHERE` does. | Caller gets a `409` instead of a queued retry. |
| **Postgres outbox for webhooks** | One-command setup; survives restart; transactional with the state change. | Redis / RabbitMQ / Kafka | New infra in `docker compose`; overkill at this scale. | Worker polls; ~1s delivery-latency floor. |
| **Webhook claim/lease (no lock during POST)** | Dead endpoint can't hold a row lock or connection; `SKIP LOCKED` lets replicas share the queue. | `FOR UPDATE SKIP LOCKED` held across the POST | Same lock-across-I/O problem as payments. | A crashed worker leaves rows `inflight` until `lease_until` expires (30s). |
| **API key `key_id` + `secret`** | Exact O(1) lookup; no prefix-collision reasoning. | 8-char prefix scan | Ambiguous multi-row lookups; collision handling. | Token is a little longer. |
| **SHA-256 for API keys** | 256-bit random secret; KDF latency buys nothing. | Argon2 / bcrypt | Per-request CPU cost defends a threat (low-entropy guessing) that doesn't exist here. | Wrong *if* a key were low-entropy — generation guarantees it isn't. |
| **Migrations on app startup** | One service, one writer, zero extra moving parts. | `migrate` container / CI gate | Unnecessary orchestration for a take-home. | Two replicas race on boot — fine single-instance; documented. |
| **`/healthz` (liveness) ≠ `/readyz` (readiness)** | A slow DB shouldn't get a healthy process killed. | Single `/health` that pings DB | Orchestrator restarts on transient DB blips. | Two endpoints to document. |
| **`tok_network_error` always 500** | Deterministic tests. | Alternate 500 / socket drop | Non-reproducible failures. | Doesn't exercise the raw socket-drop path (covered conceptually in DESIGN). |

---

## 17. What I cut (DESIGN.md §6 — required section, draft)

| Cut | Why omitted for the take-home | What a production version needs |
|---|---|---|
| **Draft invoices + editing** | No edit endpoint in scope; a state with no operations is noise. | `draft` state, line-item PATCH, `finalize` transition, "draft expires" job. |
| **Refunds & partial payments** | Explicitly out of scope. | `refunds` table, `amount_refunded_cents`, `open→partially_paid`, PSP refund API, refund webhooks. |
| **Production rate limiting** | Explicitly out of scope; would eat the budget. | Token bucket per `key_id` (Redis), per-IP connection caps, `429` + `Retry-After`, quota tiers. |
| **Broker-backed webhook queue + dunning** | Postgres outbox is sufficient at this scale; retry campaigns are a product feature. | Kafka/SQS for delivery, a dunning scheduler, campaign backoff, customer-facing retry emails. |
| **Audit log + admin tooling** | Not required; the event log covers reconciliation. | Append-only `audit_events` (who/what/when) for key rotation, manual voids, forced replay; an internal API. |
| **Full SSRF hardening** | Best-effort registration check shows awareness. | Resolve-then-pin DNS, block metadata/private ranges at connect time, no-follow-redirects, egress proxy. |

---

## 18. Correctness invariants checklist (maps to grading axes + the spec self-check)

- [ ] No `f32`/`f64`/`Decimal` anywhere under the money path — `grep` proves it; `Cents`
      has no float conversion.
- [ ] Every tenant-scoped query has `WHERE business_id = $1`; cross-tenant
      customer↔invoice link is impossible at the DB level.
- [ ] 20 concurrent `POST /pay` (same invoice) → exactly one PSP charge, one `succeeded`
      attempt, final state `paid`.
- [ ] 20 concurrent `POST /pay` with distinct keys + `tok_timeout` → exactly one PSP
      request in flight.
- [ ] `tok_timeout` → endpoint returns in ~5s with `202`; invoice never leaves `open`
      until settled; sweeper finishes it; endpoint never hangs.
- [ ] PSP success + crash before settle → sweeper re-submits idempotently → exactly one
      charge.
- [ ] Same idempotency key + different body → `409`, no PSP call.
- [ ] `POST /pay` on `paid` invoice → replay (same key) or `409` (new key); never a PSP
      call.
- [ ] Invalid state transitions → `409` with `from`/`to`.
- [ ] Webhooks: signed, off the request path, no lock held during POST, documented
      backoff, exhausted rows queryable, event log replayable.
- [ ] `docker compose up` on a clean machine → everything up, API key in logs, README
      curls pass, no manual steps.
- [ ] One error JSON shape everywhere.
- [ ] Commits small, bodies explain *why*, author is the candidate only.
- [ ] `DESIGN.md` answers (a)–(e) specifically, names the concurrency mechanism(s),
      lists 3–5 cuts, has the state diagram.
- [ ] `AI_USAGE.md` is honest and specific.
- [ ] Video link is in `README.md`, accessible without login, covers all four sections.

---

## 19. Time budget (4–6 focused hours) & degrade order

| Block | Commits | ~Time |
|---|---|---|
| Scaffold + schema + bootstrap + auth | 1–4 | 1h15 |
| Customers + invoices + state machine | 5–6 | 55m |
| Mock PSP + payment core + sweeper | 7–8 | 1h30 |
| Webhooks | 9 | 45m |
| Tests + Docker | 10–11 | 50m |
| DESIGN.md + README + OpenAPI + AI_USAGE | 12–13 | 45m |

**If time runs short, degrade in this order (and document each in the commit body +
`DESIGN.md`):**
1. Payment sweeper's "fail after max age" branch → stub + document.
2. Webhook backoff → constant 5-minute retry + document the intended schedule.
3. The 4th (concurrent-timeout) test → describe it in the README instead.

**Never cut:** the three required tests, the four payment mechanisms, `DESIGN.md` §3,
the demo video.

---

## 20. Meta-instruction

For every choice above: if, while building, an alternative turns out simpler, safer, or
more honest for a 4–6 hour take-home, **take it and record why in the commit body and
`DESIGN.md`** — don't preserve a decision just because it's written here. The bar is not
the most sophisticated service; it's a service where **every mechanism maps to one named
invariant you can explain on camera without notes.**
