# Dodo Payments Backend Take-Home — Commit Checklist

This checklist tracks the implementation of the 13 commits specified in the build prompt (`prompt.md`).

- [x] **Commit 1:** `chore: scaffold workspace, toolchain, doc skeletons`
  - Workspace `Cargo.toml` (two members: `crates/invoice-service`, `crates/mock-psp`, no `shared`)
  - `rust-toolchain.toml`, `.rustfmt.toml`, `clippy.toml`, `.gitignore`, `.dockerignore`, `.env.example`
  - `README.md`, `DESIGN.md`, `AI_USAGE.md`, `API.md`, `openapi.yaml` skeletons with required headings
  - Both `main.rs` binaries parsing config, init tracing, logging "listening", exiting clean
  - Thin `Cents` newtype in `money.rs` (`i64` minor units)

- [x] **Commit 2:** `feat: schema and migrations`
  - `migrations/0001_init.sql` containing tables: `businesses`, `api_keys`, `customers`, `invoices`, `invoice_line_items`, `payment_attempts`, `webhook_endpoints`, `webhook_events`, `webhook_deliveries`
  - Required check constraints, unique constraints, and partial unique indexes (e.g. `one_pending_payment_per_invoice`, unique idempotency key)

- [x] **Commit 3:** `feat: config, error model, bootstrap, health`
  - Hand-rolled `Config::from_env()` parsing environment variables with explicit errors
  - Single `ApiError` enum mapping to consistent JSON error responses `{"error":{"code","message","details"?}}`
  - Tracing subscriber (JSON output), `TraceLayer`, and `request_id` middleware
  - `/healthz` (liveness) and `/readyz` (readiness with DB check and migrations check)
  - Graceful shutdown on SIGTERM / Ctrl-C

- [x] **Commit 4:** `feat: API key authentication`
  - Token format: `dodo_<key_id>_<secret>`
  - `key_id` stored plaintext with unique constraint, `secret_hash` stored as SHA-256 `bytea`
  - Auth middleware extracting Bearer token, validating via constant-time hash comparison and revocation checks
  - `invoice-service seed` subcommand to provision test businesses/API keys

- [x] **Commit 5:** `feat: customers`
  - `POST /v1/customers`, `GET /v1/customers/:id`, `GET /v1/customers?limit=&cursor=` (keyset pagination)
  - Tenant isolation enforcing `WHERE business_id = $1` on every query
  - DB-level constraints preventing cross-tenant customer references

- [x] **Commit 6:** `feat: invoices with server-computed totals + state machine`
  - `POST /v1/invoices` (server computes line amounts and total using `Cents`, rejects client totals, validates quantities and line counts)
  - `GET /v1/invoices/:id`, `GET /v1/invoices?state=&limit=&cursor=`
  - State machine actions: `POST /v1/invoices/:id/void`, `POST /v1/invoices/:id/mark-uncollectible`
  - Conditional `UPDATE` enforcement (`transition_invoice` helper) and exhaustive state transition unit test

- [x] **Commit 7:** `feat: mock PSP`
  - Workspace binary `crates/mock-psp` with route `POST /charge` and debug endpoint `GET /_debug/charges`
  - Token behaviors (`tok_success`, `tok_insufficient_funds`, `tok_card_declined`, `tok_timeout`, `tok_network_error`)
  - Deterministic in-memory idempotency cache on `idempotency_key`

- [x] **Commit 8:** `feat: payment attempts — claim, call PSP, settle; + reconciliation sweeper`
  - 3-phase payment execution (`POST /v1/invoices/:id/pay` with `Idempotency-Key` header):
    1. Claim (short transaction, request fingerprinting, duplicate/concurrency checks using partial unique indexes)
    2. Call PSP with hard 5s timeout (no open transaction)
    3. Settle (short transaction updating attempt status and transitioning invoice state)
  - Concurrency guards and idempotency replay handling
  - Reconciliation sweeper background task for pending payment attempts

- [x] **Commit 9:** `feat: webhooks — events, signed delivery, claim/lease worker, retries`
  - Webhook endpoint registration (`POST /v1/webhook_endpoints`) with secret generation
  - Outbox pattern: state changes write `webhook_events` and per-endpoint `webhook_deliveries` in the same transaction
  - Claim/lease background worker delivering signed HMAC-SHA256 webhooks (`Dodo-Signature` header, timestamp + event ID replay protection) with configured backoff schedule and max attempts

- [x] **Commit 10:** `test: concurrency, idempotency, PSP-failure (+ concurrent-timeout)`
  - `concurrency.rs`: 20 concurrent payments with distinct keys resulting in exactly 1 success and 19 conflict/in-progress responses
  - `idempotency.rs`: Repeated requests with the same idempotency key replay the identical outcome
  - `psp_failure.rs`: Handles timeouts (202 response + sweeper resolution) and network errors correctly
  - State machine transition unit tests

- [ ] **Commit 11:** `chore: dockerfiles and docker-compose`
  - Multi-stage `Dockerfile` (cargo-chef caching) and committed `.sqlx/` offline data
  - `docker-compose.yml` linking Postgres DB, seed container, mock PSP, and invoice service (with automatic startup migrations)

- [ ] **Commit 12:** `docs: DESIGN.md`
  - Comprehensive ~800–1500 word design document following the spec structure:
    - Data model & Mermaid ER diagram
    - State machine diagram & transition rules
    - Payment correctness mechanisms & answering questions (a)–(e)
    - Webhook signing, replay protection, and outbox delivery design
    - API key model & rationale for SHA-256 over KDF
    - Deliberately cut features & production readiness gap analysis

- [ ] **Commit 13:** `docs: README, OpenAPI, AI_USAGE`
  - `README.md` with one-command run instructions (`docker compose up`), curl examples, test notes, and demo video link
  - `openapi.yaml` and `API.md` documenting all endpoints, request/response shapes, and error envelope
  - `AI_USAGE.md` detailing tool usage and specific design decisions made independently of or against AI suggestions
