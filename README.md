# Invoice & Payment Service

A minimal invoice & payment backend: a business authenticates with an API key,
creates customers and invoices, customers pay invoices through a mock payment
processor, and the business is notified of state changes via signed webhooks.

Built for the Dodo Payments backend take-home. Design rationale lives in
[`DESIGN.md`](DESIGN.md); AI-tool usage is disclosed in [`AI_USAGE.md`](AI_USAGE.md).

> **Status:** in progress. The run instructions and curl walkthrough below grow
> as the service comes up.

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

<!-- TODO (Commit 13): copy-paste curl flow against :8080 -->
1. Create a customer
2. Create an invoice (server computes the total)
3. Get the invoice
4. Pay it with `tok_success` → `200`, invoice `paid`
5. Pay another with `tok_card_declined` → `402`
6. `GET /v1/webhook_deliveries` to see the fan-out

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
