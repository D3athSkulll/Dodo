# Postman collection

`Invoice-Payment-Service.postman_collection.json` walks the whole API with
assertions on the expected behaviour (status codes, error envelope, state
transitions, idempotency).

## Use

1. Start the service — `docker compose up` from the repo root, or the local run
   in the main README.
2. Import both files into Postman:
   - `Invoice-Payment-Service.postman_collection.json`
   - `local.postman_environment.json`
3. Select the **Invoice Service — local** environment and set `apiKey` to the
   seeded key. With `docker compose up` it is in `docker compose logs seed`
   (the `api_key` line); locally it is printed by `cargo run -p invoice-service
   seed`.
4. Run folders top to bottom, or use the **Collection Runner** on the whole
   collection. Requests chain through collection variables, so order matters
   within a folder.

## Or from the CLI

```bash
npm install -g newman
newman run postman/Invoice-Payment-Service.postman_collection.json \
  -e postman/local.postman_environment.json \
  --env-var apiKey=dodo_xxx_yyy
```

## What it covers

| Folder | Checks |
|--------|--------|
| Health | `/healthz` 200 + `x-request-id`; `/readyz` 200 |
| Auth | missing / bad key → 401 with the error envelope |
| Customers | create 201, get 200 / 404, invalid → 422 with per-field details, keyset list |
| Invoices | server-computed total (`3297`), client `total` → 422, empty lines → 422, get with line items, list by state, void → 200 then re-void → 409 `invalid_state_transition` |
| Payments | missing `Idempotency-Key` → 422; success → 200 (`paid`); replay same key → same attempt id; pay a paid invoice → 409 `invoice_not_open`; declined → 402 (invoice stays open); same key + different token → 409 `idempotency_key_conflict`; payment read endpoints |
| Webhooks | register (secret once); blocked URL → 422 when the SSRF guard is on (201 under compose, where it is disabled); event log contains `invoice.created` + `invoice.paid`; delivery list + `?status=` filter |

Not covered here (needs true concurrency or time control — see the Rust
integration tests): 20-way concurrent `pay`, `tok_timeout` + sweeper recovery,
`tok_network_error` give-up.
