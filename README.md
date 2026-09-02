# Invoice & Payment Service

A minimal invoice & payment backend: a business authenticates with an API key,
creates customers and invoices, customers pay invoices through a mock payment
processor, and the business is notified of state changes via signed webhooks.

Built for the Dodo Payments backend take-home. Design rationale lives in
[`DESIGN.md`](DESIGN.md); AI-tool usage is disclosed in [`AI_USAGE.md`](AI_USAGE.md).

> **Status:** in progress. The run instructions and curl walkthrough below grow
> as the service comes up.

## Feature log

What each commit adds. Newest first.

Keyed by commit subject (`git log --oneline`), since hashes shift on rebase.

| Commit | Added |
|--------|-------|
| add payment attempts and reconciliation sweeper | `POST /v1/invoices/{id}/pay` (`Idempotency-Key` header required): three-phase claim / call-PSP / settle, no DB transaction around the PSP call. `GET /v1/payments/{id}`, `GET /v1/invoices/{id}/payments`. Background sweeper finishes stuck `pending` attempts by re-submitting the idempotent charge. `invoice.paid` / `invoice.payment_failed` events. Migration `0002` adds `payment_attempts.card_token`. |
| add mock PSP | `crates/mock-psp`: `POST /charge` with deterministic per-token outcomes (`tok_success`, `tok_insufficient_funds`, `tok_card_declined`, `tok_timeout`, `tok_network_error`), idempotent on `idempotency_key`, plus `GET /_debug/charges`. Delays tunable via `MOCK_PSP_DELAY_MS` / `MOCK_PSP_TIMEOUT_MS`. |
| add invoices and invoice state machine | `POST/GET/LIST /v1/invoices` with server-computed totals, `state` filter, `POST .../void` and `.../mark-uncollectible`. State machine `open` → `paid`/`void`/`uncollectible` (all terminal), enforced by a conditional `UPDATE`. `invoice.created` written to the webhook outbox in the same transaction as the insert. |
| add customers endpoints | `POST/GET/LIST /v1/customers`, business-scoped, keyset pagination with an opaque cursor. `/v1/*` now requires an API key. |
| add local dev postgres helper | `scripts/pg-dev.sh` — throwaway local Postgres (port 5433) for running the service without Docker. |
| add API key authentication | `dodo_<key_id>_<secret>` tokens, `Authorization: Bearer` middleware, `Business` extractor, `invoice-service seed` subcommand. |
| add config, error model, health checks, server bootstrap | Config from env, one JSON error shape, `/healthz` + `/readyz`, per-request id, migrations on startup, graceful shutdown. |
| add database schema | One migration: businesses, customers, invoices + line items, payment attempts, webhook events/deliveries. |
| scaffold workspace and doc skeletons | Cargo workspace, `Cents` money type, doc skeletons. |

## Run

<!-- TODO (Commit 11): one-command `docker compose up` -->

Toolchain: Rust 1.98 stable (any host). No `rust-toolchain.toml` — cargo uses
your default stable; the Docker build pins via the `rust:1.98` base image.

### Local Postgres, no Docker

```bash
psql -U postgres -f scripts/db-setup.sql          # create role + db `dodo`
cp .env.example .env                              # then set DATABASE_URL host to localhost
set -a && . ./.env && set +a
cargo run -p invoice-service seed                 # create a business + API key, prints it once
cargo run -p invoice-service                      # runs migrations on startup, then serves
```

Check it:

```bash
curl -i localhost:8080/healthz     # 200 while the process is up
curl -i localhost:8080/readyz      # 200 only while Postgres is reachable
```

Send the key as `Authorization: Bearer dodo_<key_id>_<secret>` on `/v1/*` routes.

`scripts/migrate.sh` still applies migrations with plain `psql` if you want the
schema without starting the app.

## API walkthrough (curl)

<!-- Grows per commit; final copy-paste flow lands in Commit 13. -->

```bash
KEY=dodo_...          # from `invoice-service seed`
AUTH="Authorization: Bearer $KEY"

# create a customer
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -d '{"name":"Acme","email":"ops@acme.com"}' localhost:8080/v1/customers

# get one, list with pagination
curl -s -H "$AUTH" localhost:8080/v1/customers/<id>
curl -s -H "$AUTH" 'localhost:8080/v1/customers?limit=2'
curl -s -H "$AUTH" 'localhost:8080/v1/customers?limit=2&cursor=<next_cursor>'

# create an invoice — the server computes total_cents and each line amount
curl -s -H "$AUTH" -H 'content-type: application/json' -d '{
  "customer_id": "<id>",
  "due_date": "2026-03-01",
  "line_items": [
    {"description": "Widget", "quantity": 2, "unit_amount_cents": 1500},
    {"description": "Bolt",   "quantity": 3, "unit_amount_cents": 99}
  ]
}' localhost:8080/v1/invoices

curl -s -H "$AUTH" localhost:8080/v1/invoices/<id>
curl -s -H "$AUTH" 'localhost:8080/v1/invoices?state=open'

# pay it — the Idempotency-Key header is required; retrying with the same key is safe
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: pay-1' \
  -d '{"card_token":"tok_success"}' localhost:8080/v1/invoices/<id>/pay      # -> 200, invoice paid

# a declined card
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: pay-2' \
  -d '{"card_token":"tok_card_declined"}' localhost:8080/v1/invoices/<other-id>/pay  # -> 402

curl -s -H "$AUTH" localhost:8080/v1/invoices/<id>/payments
```

Still to come: `GET /v1/webhook_deliveries` for the signed fan-out.

## Tests

<!-- TODO (Commit 10) -->
- `concurrency.rs` — N concurrent `POST /pay`, exactly one charge
- `idempotency.rs` — same key + body replays, no second PSP call
- `psp_failure.rs` — `tok_timeout` / `tok_network_error` never leave the invoice stuck

Per-handler tests are intentionally skipped — see the note in that section.

## API documentation

See [`openapi.yaml`](openapi.yaml) and [`API.md`](API.md). <!-- TODO (Commit 13) -->

## Demo Video

<!-- TODO (Commit 13): shareable link, accessible without login -->
