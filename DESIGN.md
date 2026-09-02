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

### Commit 7 — mock PSP

**What shipped**
`crates/mock-psp`: a second binary, one real route `POST /charge`, plus
`GET /_debug/charges` for tests.

**Design choices**
- *Outcome is a pure function of the card token* — deterministic, so tests are
  reproducible. `tok_network_error` is **always** an immediate HTTP 500 (no
  alternation, no socket drop); `tok_timeout` covers the slow / ambiguous shape.
- *Idempotent on `idempotency_key`* via an in-memory map. A repeat returns the
  stored outcome with no delay and no re-decision — this is what makes the
  crash-recovery story real (the service can re-submit the same charge and not
  double-charge).
- *A 500 or 422 stores nothing* — it is not a completed charge, so a retry gets a
  fresh decision. `tok_network_error` therefore fails again on retry;
  `tok_timeout` succeeds on the stored replay.
- *The `tok_timeout` delay is env-tunable* (`MOCK_PSP_TIMEOUT_MS`) so the
  PSP-failure integration test doesn't have to wait 30 real seconds.
- *In-memory only* — not durable across restarts. Fine for a mock; it isn't
  pretending to be a production PSP.
- *One route.* Reconciliation re-submits `POST /charge`; there is deliberately no
  `GET /charge/:id`.

**Verified** (curl)
Each token returns its documented shape and status (`200` succeeded/failed,
`422` unknown token, `500` network error). Replaying a key returns the identical
`psp_ref` with no delay — including `tok_timeout`, which sleeps once (~800ms with
the test delay) then replays in ~90ms. `/_debug/charges` lists the four stored
charges; the `tok_network_error` key is absent.

**Fixed while building Commit 8:** `tok_timeout` originally ran its sleep inside
the request handler, so when the client timed out and disconnected, axum
cancelled the handler and nothing was stored — the sweeper's retry then timed out
forever. Now the delay + store run in a detached `tokio::spawn`, like a real
processor that finishes a charge regardless of the caller.

---

### Commit 8 — payment attempts and the reconciliation sweeper

**The core commit.** `POST /v1/invoices/:id/pay` (requires an `Idempotency-Key`
header), the read model (`GET /v1/payments/:id`, `GET /v1/invoices/:id/payments`),
and the background sweeper. Migration `0002` adds `payment_attempts.card_token`.

**No database transaction ever wraps the PSP HTTP call.** Three phases:

1. **claim** — one short tx: `SELECT ... FOR UPDATE` the invoice, `INSERT` a
   `pending` attempt. No external I/O.
2. **call the PSP** — no tx open, hard 5s client timeout, `idempotency_key`
   forwarded.
3. **settle** — one short tx: record the outcome, `transition_invoice(open →
   paid)` on success, write the webhook event.

**Four mechanisms, each protecting one invariant**

| # | Mechanism | Invariant |
|---|-----------|-----------|
| 1 | `UNIQUE (business_id, idempotency_key)` | the same client operation runs once; retries replay |
| 2 | partial `UNIQUE (invoice_id) WHERE status='pending'` | at most one in-flight external charge per invoice, across different keys |
| 3 | conditional `UPDATE ... WHERE state='open'` | at most one `open → paid`; late winners no-op |
| 4 | PSP idempotency on `idempotency_key` | a retry after a transport-ambiguous first call does not double-charge |

**Answers to (a)–(e)** — the design is built around these:

- **(a) two clients, same invoice, same instant.** Both reach Phase 1. `FOR
  UPDATE` serialises the two selects; the first to commit its `INSERT` holds the
  only `pending` row (#2). The other's `INSERT` violates the partial unique index
  → `409 payment_in_progress`. One PSP call, one possible `open → paid` (#3).
- **(b) `tok_timeout`.** The 5s client timeout fires in Phase 2; Phase 3 takes
  the `Unavailable` branch — attempt stays `pending`, invoice stays `open`,
  response is `202 {attempt_id}` + `Retry-After`. The caller polls
  `GET /v1/payments/:id` or waits for `invoice.paid`. The sweeper re-submits the
  idempotent charge; the mock has by then stored the outcome, so it replays
  `succeeded` and the sweeper settles → `paid`.
- **(c) PSP succeeded, service crashed before Phase 3.** The `pending` row was
  committed in Phase 1, *before* the PSP call. The sweeper re-POSTs `/charge`
  with the same key; the mock returns the same `psp_ref` (#4). Phase 3 runs once
  → `paid`. Charged exactly once.
- **(d) same key, different body.** `request_fingerprint`
  (`sha256(invoice_id | card_token)`) mismatches the stored one → `409
  idempotency_key_conflict`, no PSP call, no state change.
- **(e) `POST /pay` on a `paid` invoice.** Phase 1 reads `state = 'paid'` under
  the lock. If the request's key matches the succeeded attempt → replay that
  `200`. Otherwise → `409 invoice_not_open`. Never a PSP call.

**Sweeper.** A Tokio task every `PAYMENT_SWEEP_INTERVAL_MS`. Claims `pending`
attempts idle ≥ 3s with `FOR UPDATE SKIP LOCKED`, bumps `updated_at`, commits
(releases the lock), *then* re-charges — no external I/O inside the claim tx.
Runs Phase 3 on the result. If an attempt is still failing past
`PAYMENT_PENDING_MAX_AGE_SECONDS`, it is failed with `psp_unreachable` and the
invoice stays `open` (retryable with a new key). Aborted on shutdown — it is
idempotent and resumes on the next start.

**Deviation from the plan.** The plan stored only `request_fingerprint`, not the
card token. The sweeper needs the token to re-submit `/charge` in the case where
the *first* call never reached the PSP, so `0002` adds a `card_token` column.
`request_fingerprint` is kept for case (d).

**Idempotency response model.** Operation-based, not HTTP-replay. The same key
maps to the same `payment_attempts` row; the response *evolves* (`202 pending` →
later `200 succeeded` / `402 failed`). We store `status` / `psp_ref` /
`failure_code`, enough to render any later response — not a frozen status+body.

**Verified** (two processes, dev cluster, shortened timers)
- happy path: `tok_success` → `200`, invoice `paid`, one charge, `invoice.paid`.
- idempotent replay (same key + body) → identical `200` body, no second charge.
- `tok_card_declined` → `402`, invoice stays `open`, `invoice.payment_failed`.
- missing `Idempotency-Key` → `422`.
- same key, different token → `409 idempotency_key_conflict`.
- `POST /pay` on a paid invoice, new key → `409 invoice_not_open`.
- `tok_timeout` → `202 pending`; ~6s later the sweeper settles it to `paid` with
  **exactly one** charge at the mock.
- `tok_network_error` with `PAYMENT_PENDING_MAX_AGE_SECONDS=2` → attempt failed
  `psp_unreachable`, invoice still `open`, never stuck.
- two concurrent pays, different keys → one `200` (`paid`), one `409
  payment_in_progress`; **one** `succeeded` attempt row, one charge.

---

### Commit 9 — webhooks

**What shipped**
`POST /v1/webhook_endpoints`, the delivery worker, retry/backoff, and two
reconciliation endpoints. The outbox writes were already wired in Commits 6 & 8.

**Design choices**
- *Signing:* `Dodo-Signature: t=<unix>,v1=<hex>` where `hex =
  hmac_sha256(secret, "<t>.<body>")`, symmetric per-endpoint secret. Asymmetric
  (Ed25519) rejected — key distribution overhead with no threat model that needs
  it here.
- *Replay protection is two mechanisms:* the receiver rejects if `|now - t|` is
  large **and** dedupes on `Dodo-Event-Id`. Freshness alone still allows replay
  inside the window; the event id closes it. Delivery is at-least-once by design.
- *`webhook_events` vs `webhook_deliveries`:* the payload is stored once on the
  event; a delivery is one row per (event, endpoint) carrying only attempt
  state. No payload duplication.
- *No lock during the POST.* Claim + lease in one tx, commit, POST, record the
  outcome in a second tx. A dead endpoint can never hold a row lock or a pooled
  connection. `SKIP LOCKED` lets replicas share the queue; a crashed worker's
  `inflight` rows free themselves once `lease_until` passes.
- *Decoupled from the request path.* The API handler's transaction only inserts
  delivery rows — it never makes an outbound call, so `/pay` latency is
  independent of every registered endpoint's health.
- *Backoff:* `1m, 5m, 30m, 2h, 6h`, then `exhausted` at 6 attempts (~8h46m
  budget). Retryable = timeout / connection error / 5xx / 408 / 429; any other
  4xx is permanent. Jitter is a noted production improvement.
- *Reconciliation:* `GET /v1/webhook_events` is the durable log to replay from;
  `GET /v1/webhook_deliveries?status=exhausted` is what never got through.
- *SSRF:* best-effort — parse, require http(s), resolve the host, reject
  loopback / private / link-local / metadata IPs. Gated by
  `WEBHOOK_ALLOW_PRIVATE_TARGETS` (off in prod; on for local dev and compose,
  where the receiver is a sibling on a private address). Full protection also
  needs resolve-then-pin and no-follow-redirects at connect time — DESIGN §7.
- Email is `tracing::info!("would send email …")` only.

**Deviation from the plan.** Added the `WEBHOOK_ALLOW_PRIVATE_TARGETS` flag —
without it, `docker compose` and every local demo would be unable to register a
reachable receiver.

**Verified** (worker + a Python receiver that recomputes the HMAC)
`invoice.created` and `invoice.paid` both delivered; the receiver's independent
`hmac_sha256(secret, "<t>.<body>")` matches `v1` for both; `Dodo-Event-Id`
present. With the flag off, `127.0.0.1`, `169.254.169.254`, `10.x` and non-http
schemes are all `422`; `https://example.com` is accepted. Against a dead port,
deliveries go `pending` with `attempts = 1` and `next_attempt_at` pushed out by
the backoff. `?status=exhausted` is empty on the happy path.

---

### Commit 10 — integration tests

**What shipped**
`tests/` running against real Postgres via `#[sqlx::test]` (isolated database per
test), with the real service router and the real mock PSP spun up in-process on
ephemeral ports and driven by `reqwest`. `mock-psp` became lib + bin so the
harness can embed it; `scripts/pg-dev.sh` now grants `dodo` `CREATEDB`.

**The tests**
- `concurrency` — 20 concurrent `POST /pay`, distinct keys → exactly one `200`,
  every other response is `202`/`409`, `count(succeeded) = 1`, one charge at the
  PSP, invoice `paid`.
- `idempotency` — same key + body twice → byte-identical response, `charge_count
  = 1`.
- `psp_failure`
  - `tok_timeout`: `202`, attempt `pending`, invoice `open`; the sweeper
    re-submits the idempotent charge and settles it to `paid` — still one charge.
  - `tok_network_error` with `pending_max_age = 1s`: retried, then `failed`
    (`psp_unreachable`), invoice still `open`, never stuck `pending`.
- `concurrent_timeout` (not spec-required) — 20 concurrent timeouts, distinct
  keys → one `202`, nineteen `409`, one attempt row, invoice `open`, and once the
  PSP finishes, exactly one charge. This is mechanism #2 on its own.

**Design choices**
- *In-process, not shelling out.* The service is already lib + bin; the harness
  builds `AppState` from the `#[sqlx::test]` pool directly, so there is no env or
  port juggling and no orphan processes.
- *Timings are injected, not slept through.* A `Timings` struct turns the mock's
  delays, the client PSP timeout, the sweep interval and the pending-max-age
  down so the suite runs in ~1 minute. The sweeper's 3s idle floor is real, so
  the timeout tests genuinely wait for it.
- *Per-handler HTTP tests are skipped on purpose* — the three risk areas are
  covered end to end and the handlers are thin.

**Verified** — `cargo test --workspace`: 20 unit + 5 integration, all green.

---

### Housekeeping — modularise the source tree

Pure move commit, no behaviour change. The 18 flat `src/*.rs` files became:

```text
src/
  money · config · error · telemetry · secret · pagination   cross-cutting leaves
  state.rs      AppState, the pool, the HTTP client
  auth.rs       API-key middleware + Business extractor + seed
  psp.rs        outbound payment-processor client
  routes/       HTTP handlers (customers, invoices, payments, webhooks, health)
                + mod.rs, which is the only place the router is assembled
  domain/       invoice_state (the machine) + outbox (the webhook write)
  workers/      payment_sweeper + webhook_delivery
```

`git` tracked every file as a rename, so history follows. `lib.rs` now opens with
this map so a new reader knows where to look. `app.rs` split into `state.rs`
(state) and `routes/mod.rs` (wiring).

---

### Commit 11 — Dockerfile and docker-compose

**What shipped**
A multi-stage `Dockerfile` and a `docker-compose.yml` that brings the whole
system up with one command.

**Design choices**
- *One image, both binaries.* `invoice-service` and `mock-psp` share a build;
  compose picks which with `command:`.
- *cargo-chef* caches the dependency compile as its own layer, so source edits
  rebuild in seconds.
- *rustls everywhere* (reqwest, sqlx), so the image needs no OpenSSL /
  `libssl-dev` — just `ca-certificates` for outbound HTTPS webhooks. Runtime is
  `debian:bookworm-slim`, non-root.
- *No `.sqlx` offline cache.* The plan expected one, but every query in the
  service is unchecked `sqlx::query`, so `cargo build` needs no database and
  there is nothing to prepare. Migrations are embedded by `sqlx::migrate!()` at
  compile time, so the runtime image carries no SQL files.
- *Compose:* `db` is healthchecked and `app` / `seed` wait on it; `seed` is a
  one-shot that prints the API key to its logs and exits; one named volume for
  Postgres data; all env is inline in the compose file (the `.env.example`
  values), so `docker compose up` needs nothing else. `WEBHOOK_ALLOW_PRIVATE_
  TARGETS=true` there because sibling services are on private container IPs.

**Not verified locally.** Docker is not installed on the build machine, so the
image build and `docker compose up` were not run here — the compose file is
schema-valid and the Dockerfile follows the standard cargo-chef pattern, but the
reviewer's `docker compose up` is the real check.

---

### Housekeeping — Postman collection

`postman/` holds a collection that walks the whole API with assertions on the
expected behaviour (status codes, the error envelope, state transitions,
idempotency replay, `idempotency_key_conflict`, `invoice_not_open`). Runs in the
Postman Runner or headless with `newman`. **Verified:** `newman run` against a
live local stack — 31 requests, 55 assertions, 0 failures. Concurrency and
time-dependent cases (20-way `pay`, `tok_timeout` recovery) stay in the Rust
integration tests where they can be controlled.

---

### Dev tooling — local Postgres helper  (`99f546f`)

Not part of the plan. `scripts/pg-dev.sh` runs a throwaway Postgres in
`./.pgdata` on port 5433, isolated from any system install, with fixed
credentials (`dodo`/`dodo`). Added so the service can be run and checked by hand
without Docker.
