# Testing & API reference

Every endpoint with a sample request and response, followed by the automated
test suites. Run guide is in the main [`README.md`](README.md).

The `$API_KEY` and `$OPEN_INVOICE_ID` etc. used below come from the demo seeder:

```bash
eval "$(cargo run -q -p invoice-service demo)"     # or: docker compose --profile demo run --rm demo
AUTH="Authorization: Bearer $API_KEY"
BASE=http://localhost:8080
```

---

## API reference

Base URL `http://localhost:8080`. All `/v1/*` routes require
`Authorization: Bearer dodo_<key_id>_<secret>`. Request bodies are JSON; unknown
fields are rejected.

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
curl -s $BASE/v1/customers                       # -> 401 unauthorized
curl -s -H "$AUTH" $BASE/v1/customers            # -> 200
```

### Health

```bash
curl -s -i $BASE/healthz    # 200, header: x-request-id: <uuid>
curl -s -i $BASE/readyz     # 200 while Postgres is up; 503 if it is down
```

---

## Customers

### `POST /v1/customers` → `201`

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -d '{"name":"Acme Corp","email":"ap@acme.example"}' $BASE/v1/customers
```
```json
{ "id": "01a0601c-b323-7473-a3e9-6d5c948aed90", "name": "Acme Corp",
  "email": "ap@acme.example", "created_at": "2026-06-01T12:00:00Z" }
```

Bad input → `422`:

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -d '{"name":"  ","email":"not-an-email"}' $BASE/v1/customers
```
```json
{ "error": { "code": "validation_error", "message": "one or more fields are invalid",
  "details": [ { "field": "name",  "message": "must not be empty" },
               { "field": "email", "message": "must be a valid email address" } ] } }
```

### `GET /v1/customers/{id}` → `200`

`404` if the id is unknown or belongs to another business.

### `GET /v1/customers?limit=2` → `200`

```json
{ "data": [ { "id": "...", "name": "...", "email": "...", "created_at": "..." } ],
  "next_cursor": "MTc4ODMwMjIzNzcxMzkwNzAwMF8wMWEwNWYxZS1hMDEwLTcwZTI..." }
```

Follow the cursor: `curl -s -H "$AUTH" "$BASE/v1/customers?limit=2&cursor=<next_cursor>"`.

---

## Invoices

### `POST /v1/invoices` → `201`

The server computes each line's `amount_cents` (`unit_amount_cents × quantity`)
and `total_cents` (their sum) with checked integer arithmetic; a client-supplied
`total` is rejected.

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' -d '{
  "customer_id": "'"$CUSTOMER_ID"'",
  "due_date": "2026-06-01",
  "line_items": [
    { "description": "Widget", "quantity": 2, "unit_amount_cents": 1500 },
    { "description": "Bolt",   "quantity": 3, "unit_amount_cents": 99 }
  ]
}' $BASE/v1/invoices
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

Rejected (all `422`): empty `line_items`, `quantity < 1`, negative
`unit_amount_cents`, more than 500 lines, a bad `due_date`, an amount/total that
overflows `i64`, any unknown field such as `total_cents`.

### `GET /v1/invoices/{id}` → `200`

Returns the invoice with its `line_items`.

### `GET /v1/invoices?state=open` → `200`

List envelope (no line items). `state` is optional and one of
`open` / `paid` / `void` / `uncollectible`.

### `POST /v1/invoices/{id}/void` · `POST /v1/invoices/{id}/mark-uncollectible`

`200` with the updated invoice on success. `409 invalid_state_transition` if the
invoice is not `open`:

```bash
curl -s -H "$AUTH" -X POST $BASE/v1/invoices/<already-void-id>/void
```
```json
{ "error": { "code": "invalid_state_transition",
             "message": "invalid state transition from void to void" } }
```

### State machine

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

No transition is reversible. Every other pair is `409`. Enforced by a conditional
`UPDATE ... WHERE state = ANY($allowed_from)` — no triggers.

---

## Payments

### `POST /v1/invoices/{id}/pay`

Header `Idempotency-Key: <key>` is **required** (`422` without it). Body:
`{ "card_token": "<token>" }`.

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
  -d '{"card_token":"tok_success"}' $BASE/v1/invoices/$OPEN_INVOICE_ID/pay
```
```json
{ "attempt": { "id": "01a0601c-...", "status": "succeeded", "psp_ref": "01a0601c-..." },
  "invoice": { "id": "01a0601c-b32a-73c3-8c01-3eae28da521f", "state": "paid" } }
```

Declined:

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' -H 'Idempotency-Key: pay-2' \
  -d '{"card_token":"tok_card_declined"}' $BASE/v1/invoices/<open-id>/pay
```
```json
{ "attempt": { "id": "01a0601c-...", "status": "failed", "failure_code": "card_declined" } }
```

### `GET /v1/payments/{id}` · `GET /v1/invoices/{id}/payments`

How a caller learns the eventual result of a `202`:

```json
{ "id": "01a0601c-...", "invoice_id": "01a0601c-...", "status": "succeeded",
  "psp_ref": "psp_demo_ref_0001", "amount_cents": 12000,
  "created_at": "...", "updated_at": "..." }
```

The list form returns `{ "data": [ ...that shape... ] }`.

Payment correctness — the three-phase claim / call / settle, the four mechanisms
that prevent a double charge, and the answers to the "what happens if…" cases —
is in [`DESIGN.md`](DESIGN.md).

---

## Webhooks

### `POST /v1/webhook_endpoints` → `201`

The `secret` is shown once.

```bash
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -d '{"url":"https://your-receiver.example/hook"}' $BASE/v1/webhook_endpoints
```
```json
{ "id": "01a0...", "url": "https://your-receiver.example/hook", "secret": "720aa0bb7fb2..." }
```

URLs resolving to loopback / private / link-local / cloud-metadata addresses are
rejected (`422`) unless `WEBHOOK_ALLOW_PRIVATE_TARGETS=true`.

### Delivery

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

### `GET /v1/webhook_events` · `GET /v1/webhook_deliveries?status=exhausted`

`webhook_events` is the durable log to replay from; the `?status=exhausted`
delivery filter is what never got through.

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
