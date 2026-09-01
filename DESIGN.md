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
