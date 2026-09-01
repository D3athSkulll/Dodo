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

## Build notes (raw — turn into prose later, not final)

Short bullets logged per commit as decisions are actually made. Delete this
section before submission.

**Commit 1 — scaffold**
- Two crates, no `shared`: only shared types are a tiny PSP request/response pair.
- Deps pinned once in the workspace table; crates enable them per-commit so no
  intermediate commit carries unused-dependency noise.
- `Cents(i64)` newtype: only `checked_add`, `checked_mul_qty(u32)`, `try_sum`.
  No `Div`, no float, no dollar `Display`. `try_sum` → `None` on overflow for the
  money path; a separate saturating `impl Sum` for tests/logging only.

**Commit 2 — schema**
- One migration, all tables. Migrations run at app startup (single service,
  single writer) — a separate migrate step is a production concern, §7.
- `state`/`status` as `TEXT + CHECK`, not PG `ENUM`: CHECK is trivially altered
  later; ENUM value adds + ordering are footguns.
- Cross-tenant integrity enforced at the DB: `customers UNIQUE (id, business_id)`
  + `invoices` composite FK `(customer_id, business_id)`. Verified: an invoice
  referencing another tenant's customer is rejected by the FK.
- `one_pending_payment_per_invoice` partial unique index = the concurrency
  invariant (≤1 in-flight charge per invoice, across different keys). Verified: a
  2nd `pending` row for the same invoice is rejected.
- `payment_attempts UNIQUE (business_id, idempotency_key)` = client-op dedupe.
- Webhooks split: `webhook_events` holds the payload once; `webhook_deliveries`
  is one row per (event, endpoint) with `lease_until` for the claim/lease worker.
- Every index written against a concrete query (list customers, list invoices by
  state, poll due deliveries, replay event log).
- 100x: webhook tables are write-heavy → time-partition + retention, then a real
  queue. (§1 / §7 material.)
- Removed `rust-toolchain.toml` added in Commit 1: pinning `1.98.0` resolved to
  the MSVC host on a Windows box whose working toolchain is GNU, breaking the
  build. Version is now documented in the README; Docker pins via `rust:1.98`.

**Commit 3 — config, errors, bootstrap, health**
- `Config::from_env()` hand-rolled, ~60 lines, one typed error with the offending
  key name. No config framework.
- One `ApiError` enum → one JSON shape `{"error":{"code","message","details"?}}`.
  `Internal` logs the real cause and returns an opaque body. Validation is 422
  (request parsed, semantically rejected), not 400.
- `/healthz` = liveness, never touches the DB. `/readyz` = runs `SELECT 1`, 503
  if the DB is down. Verified: with Postgres stopped, `/healthz` stays 200 and
  `/readyz` returns 503.
- `request_id` middleware: reuse an incoming `x-request-id` or mint a UUID v7;
  put it on the tracing span and echo it on the response. Stays in logs only.
- Migrations run via `sqlx::migrate!()` on startup; graceful shutdown on
  Ctrl-C / SIGTERM.
- `readyz` uses an unchecked `sqlx::query("SELECT 1")` so `cargo build` needs no
  database. Compile-time-checked `query!` + a committed `.sqlx` cache come in
  with the first real queries (Commit 5).

**Commit 4 — API key auth**
- Token `dodo_<key_id>_<secret>`. `key_id` unique-indexed → auth is one row, no
  prefix scan. Store `key_id` plaintext + `sha256(secret)` only.
- SHA-256, not Argon2/bcrypt: the secret is 256 random bits, so a KDF just adds
  per-request latency and defends a threat (low-entropy guessing) that does not
  exist. (AI_USAGE section 2 item.)
- Constant-time compare (`subtle`) of the *hashes* — a timing leak would expose
  only bits of `sha256(guess)`, but it is free insurance.
- Revocation is `revoked_at` (soft) → audit trail survives; the lookup checks
  `revoked_at IS NOT NULL`.
- `key_id`/`secret` are hex, not base62 — a bit longer, but no bignum encoder or
  extra dep, and hex never contains `_` so token splitting stays trivial. Entropy
  is identical (96 / 256 bits).
- `Business` extractor pulls the id the middleware put in request extensions.
- `invoice-service seed` creates one business + key and prints the token once.
  Verified against a real Postgres: two runs make two businesses, `secret_hash`
  is 32 bytes, `revoked_at` null.
- Queries here are still unchecked `sqlx::query` (see Commit 3 note).
