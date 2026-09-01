# Invoice & Payment Service

A minimal invoice & payment backend: a business authenticates with an API key,
creates customers and invoices, customers pay invoices through a mock payment
processor, and the business is notified of state changes via signed webhooks.

Built for the Dodo Payments backend take-home. Design rationale lives in
[`DESIGN.md`](DESIGN.md); AI-tool usage is disclosed in [`AI_USAGE.md`](AI_USAGE.md).

> **Status:** scaffolding (Commit 1). Run instructions and the curl walkthrough
> below are filled in as the service comes up (Commits 3–11).

## Run

<!-- TODO (Commit 11): one-command run -->
```bash
docker compose up
```

The seeded API key is printed once by the `seed` service — copy it from that
service's logs.

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
