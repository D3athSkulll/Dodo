# Invoice & Payment Service

A small billing backend. A business authenticates with an API key, creates
customers and invoices, invoices are paid through a mock payment processor, and
the business is notified of state changes through signed webhooks.

Built for the Dodo Payments backend take-home. This README is the run guide and
the API reference. The design write-up is in [`DESIGN.md`](DESIGN.md); AI-tool
usage is disclosed in [`AI_USAGE.md`](AI_USAGE.md).

- **Language / stack:** Rust · Axum 0.8 · Tokio · SQLx (raw SQL, no ORM) ·
  PostgreSQL 16
- **Two binaries:** `invoice-service` and a stand-in `mock-psp`, in one Cargo
  workspace
- **Money:** integer minor units (`i64` cents) everywhere — no floats in the
  money path

---

## Run

### Docker (one command)

```bash
docker compose up --build
docker compose logs seed          # copy the api_key line
curl -i localhost:8080/healthz    # 200
```

`docker compose up` starts Postgres, the mock PSP, and the service; migrations
run on boot. The one-shot `seed` container creates a business + API key and
prints it, then exits.

To also load sample data (customers, invoices in every state, payments, webhook
events):

```bash
docker compose --profile demo run --rm demo
```

### Without Docker

Needs a local PostgreSQL. `scripts/pg-dev.sh` runs a throwaway one on port 5433,
isolated from any system install:

```bash
scripts/pg-dev.sh init                              # one-time: cluster + role + db `dodo`
cp .env.example .env                                # then set DATABASE_URL host to localhost:5433
set -a && . ./.env && set +a

cargo run -p invoice-service demo                   # sample data + copy-paste `export` lines
cargo run -p invoice-service                        # migrations on startup, then serves
```

`invoice-service seed` prints just a business + key if you want an empty
database. `scripts/migrate.sh` applies migrations with plain `psql` without
starting the app.

Toolchain: Rust 1.98 stable, any host (no `rust-toolchain.toml`; the Docker
build pins via its base image).

### Configuration

All via environment variables. `.env.example` has working defaults for
`docker compose`.

| Variable | Meaning |
|---|---|
| `DATABASE_URL` | Postgres connection string |
| `PSP_BASE_URL` | base URL of the payment processor (the mock) |
| `PSP_TIMEOUT_MS` | hard timeout on a PSP call (`5000`) |
| `BIND_ADDR` | listen address (`0.0.0.0:8080`) |
| `WEBHOOK_WORKER_INTERVAL_MS` / `WEBHOOK_LEASE_SECONDS` | delivery worker cadence / claim lease |
| `PAYMENT_SWEEP_INTERVAL_MS` / `PAYMENT_PENDING_MAX_AGE_SECONDS` | reconciliation sweeper cadence / give-up age |
| `WEBHOOK_ALLOW_PRIVATE_TARGETS` | allow webhook URLs on loopback / private ranges. `false` in production; `true` for local dev and compose |
| `RUST_LOG` | log filter (`info,invoice_service=debug`) |

---

## Quickstart

```bash
# 1. bring it up (Docker) and seed sample data
docker compose up --build -d
docker compose --profile demo run --rm demo        # prints: export API_KEY=... OPEN_INVOICE_ID=... etc
eval "$(docker compose --profile demo run --rm demo)"   # or paste the exports by hand

AUTH="Authorization: Bearer $API_KEY"

# 2. read the seeded data
curl -s -H "$AUTH" localhost:8080/v1/customers
curl -s -H "$AUTH" 'localhost:8080/v1/invoices?state=paid'
curl -s -H "$AUTH" localhost:8080/v1/invoices/$OPEN_INVOICE_ID

# 3. pay the open invoice (Idempotency-Key is required)
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: q-1' \
  -d '{"card_token":"tok_success"}' localhost:8080/v1/invoices/$OPEN_INVOICE_ID/pay
# -> 200 {"attempt":{...,"status":"succeeded",...},"invoice":{...,"state":"paid"}}

# retry with the same key — same response, no second charge
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: q-1' \
  -d '{"card_token":"tok_success"}' localhost:8080/v1/invoices/$OPEN_INVOICE_ID/pay

# 4. see the webhook events that produced
curl -s -H "$AUTH" localhost:8080/v1/webhook_events
```

---

## API reference

Base URL `http://localhost:8080`. All `/v1/*` routes require
`Authorization: Bearer dodo_<key_id>_<secret>`. Request bodies are JSON;
unknown fields are rejected.

### Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/healthz` | Liveness. `200` whenever the process is up (no DB check). |
| `GET` | `/readyz` | Readiness. `200` only while Postgres is reachable, else `503`. |
| `POST` | `/v1/customers` | Create a customer. |
| `GET` | `/v1/customers/{id}` | Get a customer. |
| `GET` | `/v1/customers?limit=&cursor=` | List customers, newest first (keyset). |
| `POST` | `/v1/invoices` | Create an invoice; the server computes every amount. |
| `GET` | `/v1/invoices/{id}` | Get an invoice with its line items. |
| `GET` | `/v1/invoices?state=&limit=&cursor=` | List invoices, optional `state` filter. |
| `POST` | `/v1/invoices/{id}/void` | `open → void`. |
| `POST` | `/v1/invoices/{id}/mark-uncollectible` | `open → uncollectible`. |
| `POST` | `/v1/invoices/{id}/pay` | Attempt payment. Requires an `Idempotency-Key` header. |
| `GET` | `/v1/invoices/{id}/payments` | All payment attempts for an invoice. |
| `GET` | `/v1/payments/{id}` | One payment attempt. |
| `POST` | `/v1/webhook_endpoints` | Register an endpoint URL; the signing secret is returned once. |
| `GET` | `/v1/webhook_events?limit=&cursor=` | The durable event log — replay from here. |
| `GET` | `/v1/webhook_deliveries?status=` | Delivery attempts; filter by `pending`/`inflight`/`delivered`/`exhausted`. |

### Error format

Every error is the same shape:

```json
{ "error": { "code": "validation_error", "message": "one or more fields are invalid",
             "details": [ { "field": "email", "message": "must be a valid email address" } ] } }
```

| Status | `code` | When |
|---|---|---|
| `401` | `unauthorized` | missing / bad / revoked API key |
| `404` | `not_found` | resource not found for this business |
| `422` | `validation_error` | body failed validation (`details` lists the fields) |
| `409` | `invalid_state_transition` | e.g. voiding an already-void invoice |
| `409` | `idempotency_key_conflict` | key reused with a different request body |
| `409` | `payment_in_progress` | another payment is already in flight for this invoice |
| `409` | `invoice_not_open` | paying an invoice that is not `open` |
| `502` | `psp_unavailable` | (reserved) |
| `500` | `internal` | opaque; the cause is only in the logs |

Bodies that fail to parse (bad JSON, an unknown field) return `422` from the
framework with a plain-text body rather than this envelope.

### Pagination

List endpoints return `{ "data": [...], "next_cursor": "<opaque>" | null }`.
Ordering is `(created_at DESC, id DESC)`. Pass `next_cursor` back as `?cursor=`
for the next page; `limit` is clamped to `1..=100` (default 20).

### Authentication

`invoice-service seed` / `demo` prints a token `dodo_<key_id>_<secret>`. Only
`key_id` (plaintext, unique) and `sha256(secret)` are stored. Send it as
`Authorization: Bearer <token>`.

```bash
curl -s localhost:8080/v1/customers                          # -> 401 unauthorized
curl -s -H "Authorization: Bearer $API_KEY" localhost:8080/v1/customers   # -> 200
```

---

### Customers

**`POST /v1/customers`** → `201`

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -d '{"name":"Acme Corp","email":"ap@acme.example"}' localhost:8080/v1/customers
```
```json
{ "id": "01a0601c-b323-7473-a3e9-6d5c948aed90", "name": "Acme Corp",
  "email": "ap@acme.example", "created_at": "2026-06-01T12:00:00Z" }
```

Bad input → `422`:

```json
{ "error": { "code": "validation_error", "message": "one or more fields are invalid",
  "details": [ { "field": "name",  "message": "must not be empty" },
               { "field": "email", "message": "must be a valid email address" } ] } }
```

**`GET /v1/customers/{id}`** → `200`, or `404` if it isn't this business's.

**`GET /v1/customers?limit=2`** → `200`

```json
{ "data": [ { "id": "...", "name": "...", "email": "...", "created_at": "..." } ],
  "next_cursor": "MTc4ODMwMjIzNzcxMzkwNzAwMF8wMWEwNWYxZS1hMDEwLTcwZTI..." }
```

### Invoices

**`POST /v1/invoices`** → `201`. The server computes each line's `amount_cents`
(`unit_amount_cents × quantity`) and `total_cents` (their sum) with checked
integer arithmetic; a client-supplied `total` is rejected.

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' -d '{
  "customer_id": "01a0601c-b323-7473-a3e9-6d5c948aed90",
  "due_date": "2026-06-01",
  "line_items": [
    { "description": "Widget", "quantity": 2, "unit_amount_cents": 1500 },
    { "description": "Bolt",   "quantity": 3, "unit_amount_cents": 99 }
  ]
}' localhost:8080/v1/invoices
```
```json
{ "id": "01a0601c-b32a-73c3-8c01-3eae28da521f",
  "customer_id": "01a0601c-b323-7473-a3e9-6d5c948aed90",
  "state": "open", "total_cents": 3297, "currency": "USD",
  "due_date": "2026-06-01", "created_at": "2026-06-01T12:00:00Z",
  "line_items": [
    { "description": "Widget", "quantity": 2, "unit_amount_cents": 1500, "amount_cents": 3000 },
    { "description": "Bolt",   "quantity": 3, "unit_amount_cents": 99,   "amount_cents": 297 } ] }
```

Rejected: empty `line_items`, `quantity < 1`, negative `unit_amount_cents`, more
than 500 lines, a bad `due_date`, an amount/total that overflows `i64`, any
unknown field (`total_cents`, …) — all `422`.

**`GET /v1/invoices/{id}`** → `200` with `line_items`.
**`GET /v1/invoices?state=open`** → `200`, list envelope (no line items).
**`POST /v1/invoices/{id}/void`** / **`.../mark-uncollectible`** → `200` with the
updated invoice; `409 invalid_state_transition` if it isn't `open`:

```json
{ "error": { "code": "invalid_state_transition",
             "message": "invalid state transition from void to void" } }
```

#### State machine

```
              ┌────────┐
              │  open  │   ← created here; the only entry point
              └───┬────┘
   payment ok     │   void            mark-uncollectible
        ┌─────────┼───────────────┐
        ▼         ▼               ▼
   ┌────────┐ ┌────────┐ ┌───────────────┐
   │  paid  │ │  void  │ │ uncollectible │     all terminal
   └────────┘ └────────┘ └───────────────┘
```

| From | To | Trigger |
|---|---|---|
| `open` | `paid` | a payment attempt succeeds |
| `open` | `void` | `POST /void` |
| `open` | `uncollectible` | `POST /mark-uncollectible` |

No transition is reversible. Every other pair is rejected with `409`. Enforced by
a conditional `UPDATE ... WHERE state = ANY($allowed_from)` — no triggers.

### Payments

**`POST /v1/invoices/{id}/pay`** — header `Idempotency-Key: <key>` is **required**
(`422` without it). Body: `{ "card_token": "<token>" }`.

Card tokens the mock PSP understands: `tok_success`, `tok_insufficient_funds`,
`tok_card_declined`, `tok_timeout` (slow), `tok_network_error` (always 5xx).

| Result | Status | Body |
|---|---|---|
| succeeded | `200` | `{"attempt":{"id","status":"succeeded","psp_ref"},"invoice":{"id","state":"paid"}}` |
| declined | `402` | `{"attempt":{"id","status":"failed","failure_code":"card_declined"}}` |
| PSP slow / unreachable | `202` + `Retry-After: 5` | `{"attempt_id","status":"pending"}` — the sweeper finishes it |
| retry, same key + body | as the original | same attempt id; no second charge |
| retry, same key, different body | `409` | `idempotency_key_conflict` |
| another payment already in flight | `409` | `payment_in_progress` |
| invoice already `paid` (new key) | `409` | `invoice_not_open` |

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: pay-1' \
  -d '{"card_token":"tok_success"}' localhost:8080/v1/invoices/$OPEN_INVOICE_ID/pay
```
```json
{ "attempt": { "id": "01a0601c-...", "status": "succeeded", "psp_ref": "01a0601c-..." },
  "invoice": { "id": "01a0601c-b32a-73c3-8c01-3eae28da521f", "state": "paid" } }
```

**`GET /v1/payments/{id}`** and **`GET /v1/invoices/{id}/payments`** — how a
caller learns the eventual result of a `202`:

```json
{ "id": "01a0601c-...", "invoice_id": "01a0601c-...", "status": "succeeded",
  "psp_ref": "psp_demo_ref_0001", "amount_cents": 12000,
  "created_at": "...", "updated_at": "..." }
```

Payment correctness (three-phase claim / call / settle, the four mechanisms that
prevent a double charge, and the answers to the "what happens if…" cases) is in
[`DESIGN.md`](DESIGN.md).

### Webhooks

**`POST /v1/webhook_endpoints`** → `201`. The `secret` is shown once.

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -d '{"url":"https://your-receiver.example/hook"}' localhost:8080/v1/webhook_endpoints
```
```json
{ "id": "01a0...", "url": "https://your-receiver.example/hook", "secret": "720aa0bb7fb2..." }
```

URLs resolving to loopback / private / link-local / cloud-metadata addresses are
rejected (`422`) unless `WEBHOOK_ALLOW_PRIVATE_TARGETS=true`.

Events emitted: `invoice.created`, `invoice.paid`, `invoice.payment_failed` —
each written to the outbox in the **same transaction** as the state change. A
background worker delivers them, off the request path, retrying on failure with
`1m, 5m, 30m, 2h, 6h` backoff to `exhausted` after 6 attempts.

Each POST carries:

```
Content-Type: application/json
Dodo-Event-Id: <uuid>
Dodo-Signature: t=<unix-seconds>,v1=<hex hmac_sha256(secret, "<t>.<raw body>")>
```

Verify:

```python
import hmac, hashlib
t, v1 = parse("Dodo-Signature")          # "t=...,v1=..."
expected = hmac.new(secret.encode(), f"{t}.{raw_body}".encode(), hashlib.sha256).hexdigest()
assert hmac.compare_digest(expected, v1) and abs(now() - int(t)) < 300
# also dedupe on Dodo-Event-Id — delivery is at-least-once
```

**`GET /v1/webhook_events`** is the durable log to replay from;
**`GET /v1/webhook_deliveries?status=exhausted`** is what never got through.

```json
{ "data": [
    { "id": "01a0...", "event_type": "invoice.paid",
      "resource_id": "01a0601c-b333-...", "payload": { "type": "invoice.paid", "...": "..." },
      "created_at": "..." } ],
  "next_cursor": null }
```

---

## Tests

```bash
scripts/pg-dev.sh start
DATABASE_URL=postgres://dodo:dodo@localhost:5433/dodo cargo test --workspace
```

Each integration test gets its own database (`#[sqlx::test]`) and runs the real
service + mock PSP in-process on ephemeral ports.

| Test | What it proves |
|---|---|
| `concurrency.rs` | 20 concurrent `POST /pay`, distinct keys → exactly one `200`, one `succeeded` attempt, **one** charge at the PSP, invoice `paid`. |
| `idempotency.rs` | Same key + body twice → byte-identical response, no second PSP call. |
| `psp_failure.rs` | `tok_timeout` → `202`, the sweeper re-submits and settles it to `paid` (one charge). `tok_network_error` → retried, then `failed` (`psp_unreachable`), invoice stays `open` — never stuck `pending`. |
| `concurrent_timeout.rs` | 20 concurrent timeouts, distinct keys → one `202`, nineteen `409`, one attempt row, one charge once the PSP finishes. |

Unit tests cover: `Cents` overflow boundaries, the `ApiError` → status map, the
exhaustive invoice state-transition table, API-key token parsing, the webhook
backoff schedule and signature, the SSRF address filter, pagination cursor
round-trips.

**Results:** `cargo test --workspace` → **20 unit + 5 integration, all passing**.
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check` are clean.

Per-handler HTTP tests are intentionally skipped: the three risk areas
(concurrency, idempotency, PSP failure) are covered end to end, and the handlers
are thin wrappers over the pieces those tests already exercise.

### Postman

[`postman/`](postman/) has a collection that walks the whole API with assertions
(status codes, the error envelope, state transitions, idempotency replay and
conflict). Import it with `postman/local.postman_environment.json`, or run it
headless:

```bash
newman run postman/Invoice-Payment-Service.postman_collection.json \
  --env-var baseUrl=http://localhost:8080 --env-var apiKey=<seeded key>
```

Last run: **31 requests, 55 assertions, 0 failures.**

---

## Project layout

```
crates/
  invoice-service/
    src/
      money · config · error · telemetry · secret · pagination   cross-cutting leaves
      state.rs        AppState, the pool, the HTTP client
      auth.rs         API-key middleware + Business extractor + seed
      psp.rs          outbound payment-processor client
      demo.rs         `invoice-service demo` sample data
      routes/         HTTP handlers (customers, invoices, payments, webhooks, health) + router()
      domain/         invoice_state (the state machine) · outbox (the webhook write)
      workers/        payment_sweeper · webhook_delivery
    migrations/       0001_init.sql, 0002_payment_card_token.sql
    tests/            concurrency · idempotency · psp_failure · concurrent_timeout
  mock-psp/           the stand-in payment processor (lib + bin)
scripts/              pg-dev.sh · db-setup.sql · migrate.sh
postman/              collection + environment
```

---

## Feature log

Newest first. Keyed by commit subject (`git log --oneline`), since hashes shift
on rebase.

| Commit | Added |
|--------|-------|
| add demo data seeder | `invoice-service demo` populates one business + key, 3 customers, an invoice in every state with line items and payment attempts, a webhook endpoint, and the resulting events/deliveries — so every read route returns real data immediately. Also a `demo` compose profile. |
| add Postman collection | `postman/` — a collection that walks the whole API with assertions (status, error envelope, state transitions, idempotency). Runs via the Postman Runner or `newman`; 31 requests / 55 assertions. |
| add Dockerfile and docker-compose | Multi-stage `Dockerfile` (cargo-chef dep caching, rustls, non-root) building both binaries into one image. `docker-compose.yml`: `db` (healthchecked), `mock-psp`, `seed` (one-shot, logs the key), `app` (port 8080), one named volume, all env inline. |
| modularise the source tree | No behaviour change. Flat `src/*.rs` grouped into `routes/`, `domain/`, `workers/`; cross-cutting leaves at the root. `lib.rs` opens with a map of the layout. |
| add integration tests | `tests/` on real Postgres (`#[sqlx::test]`, isolated DB per test) running the app + mock PSP in-process: `concurrency`, `idempotency`, `psp_failure`, `concurrent_timeout`. `mock-psp` is now lib + bin. |
| add webhooks | `POST /v1/webhook_endpoints` (best-effort SSRF guard, secret once). Delivery worker (claim/lease, no lock during POST) signs each payload — `Dodo-Signature` + `Dodo-Event-Id` — and retries with `1m,5m,30m,2h,6h` backoff to `exhausted` at 6 attempts. Reconciliation via `GET /v1/webhook_events` and `?status=` on deliveries. |
| add payment attempts and reconciliation sweeper | `POST /v1/invoices/{id}/pay` (`Idempotency-Key` required): three-phase claim / call-PSP / settle, no DB transaction around the PSP call. Read model at `GET /v1/payments/{id}` and `.../payments`. Sweeper finishes stuck `pending` attempts. Migration `0002` adds `payment_attempts.card_token`. |
| add mock PSP | `crates/mock-psp`: `POST /charge` with deterministic per-token outcomes, idempotent on `idempotency_key`, plus `GET /_debug/charges`. |
| add invoices and invoice state machine | `POST/GET/LIST /v1/invoices` with server-computed totals, `state` filter, `void` and `mark-uncollectible`. State machine enforced by a conditional `UPDATE`. `invoice.created` to the outbox in the same transaction. |
| add customers endpoints | `POST/GET/LIST /v1/customers`, business-scoped, keyset pagination. `/v1/*` now requires an API key. |
| add local dev postgres helper | `scripts/pg-dev.sh` — throwaway local Postgres (port 5433). |
| add API key authentication | `dodo_<key_id>_<secret>` tokens, `Authorization: Bearer` middleware, `Business` extractor, `invoice-service seed`. |
| add config, error model, health checks, server bootstrap | Config from env, one JSON error shape, `/healthz` + `/readyz`, per-request id, migrations on startup, graceful shutdown. |
| add database schema | One migration: businesses, customers, invoices + line items, payment attempts, webhook events/deliveries. |
| scaffold workspace and doc skeletons | Cargo workspace, `Cents` money type, doc skeletons. |

---

## Design & AI usage

- [`DESIGN.md`](DESIGN.md) — data model, state machine, payment correctness and
  failure modes, webhook design, API-key model, what was cut, production gaps.
- [`AI_USAGE.md`](AI_USAGE.md) — which tools were used where, decisions made
  independently, and one thing that had to be corrected.

## Demo video

_TODO: shareable link, accessible without login._
