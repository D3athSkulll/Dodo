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

| Commit | Added |
|--------|-------|
| `63606b8` | Customers: `POST/GET/LIST /v1/customers`, business-scoped, keyset pagination with opaque cursor. `/v1/*` now requires an API key. |
| `99f546f` | `scripts/pg-dev.sh` — throwaway local Postgres (port 5433) for running the service without Docker. |
| `6e2795c` | API key auth: `dodo_<key_id>_<secret>` tokens, `Authorization: Bearer` middleware, `Business` extractor, `invoice-service seed` subcommand. |
| `5df50a8` | Config from env, one JSON error shape, `/healthz` + `/readyz`, per-request id, migrations on startup, graceful shutdown. |
| `da429b7` | Full database schema (one migration): businesses, customers, invoices + line items, payment attempts, webhook events/deliveries. |
| `990130b` | Workspace scaffold, `Cents` money type, doc skeletons. |

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
```

Still to come: create an invoice (server computes the total), get it, pay with
`tok_success` → `paid`, pay with `tok_card_declined` → `402`, then
`GET /v1/webhook_deliveries` for the fan-out.

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
