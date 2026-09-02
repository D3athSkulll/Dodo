# Invoice & Payment Service

A small billing backend. A business authenticates with an API key, creates
customers and invoices, invoices are paid through a mock payment processor, and
the business is notified of state changes through signed webhooks.

Built for the Dodo Payments backend take-home.

- **Stack:** Rust · Axum 0.8 · Tokio · SQLx (raw SQL, no ORM) · PostgreSQL 16
- **Two binaries:** `invoice-service` and a stand-in `mock-psp`, in one Cargo
  workspace
- **Money:** integer minor units (`i64` cents) everywhere — no floats in the
  money path

**Docs:** [`TESTING.md`](TESTING.md) is the full API reference with sample
requests and responses, plus how to run the test suites. Design rationale is in
[`DESIGN.md`](DESIGN.md); AI-tool usage in [`AI_USAGE.md`](AI_USAGE.md).

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
# bring it up and seed sample data
docker compose up --build -d
eval "$(docker compose --profile demo run --rm demo)"   # export API_KEY=... OPEN_INVOICE_ID=... etc

AUTH="Authorization: Bearer $API_KEY"

# read the seeded data
curl -s -H "$AUTH" localhost:8080/v1/customers
curl -s -H "$AUTH" 'localhost:8080/v1/invoices?state=paid'

# pay the open invoice (the Idempotency-Key header is required)
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: q-1' \
  -d '{"card_token":"tok_success"}' localhost:8080/v1/invoices/$OPEN_INVOICE_ID/pay
# -> 200, invoice now "paid"; retry with the same key for an identical response and no second charge

curl -s -H "$AUTH" localhost:8080/v1/webhook_events
```

Full request/response examples for every endpoint are in
[`TESTING.md`](TESTING.md).

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

## Tests

```bash
scripts/pg-dev.sh start
DATABASE_URL=postgres://dodo:dodo@localhost:5433/dodo cargo test --workspace
```

**Results:** 20 unit + 5 integration tests passing; `cargo clippy --workspace
--all-targets` and `cargo fmt --all --check` clean; the Postman collection is
31 requests / 55 assertions / 0 failures. What each test proves, and how to run
the Postman collection, is in [`TESTING.md`](TESTING.md#tests).

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
