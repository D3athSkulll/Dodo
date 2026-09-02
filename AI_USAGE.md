# AI_USAGE

Honest, specific disclosure of how AI tools were used on this take-home. Graded.

> **Status:** skeleton (Commit 1). Filled in throughout; finalised in Commit 13
> with what actually happened during the build.

## 1. Which AI tools, and where

Used **Claude Sonnet 5** to draft the initial plan for handling the task.
Used **ChatGPT 5.0** to think and ponder over the initial plan and list the architectural gaps and alternatives to the plan.
Used **Claude Sonnet 5** for improving the original plan based on ChatGPT's review and create a final plan for executing the task. 

## 2. Three decisions I made myself (against or independent of AI suggestions)

<!-- For each: what the AI proposed (if anything), what I chose, and why. Draft: -->

## 3. One thing the AI got wrong, or that I had to correct

AI made the issue in building the workspace. The system has Windows MSVC toolchain along with GNU toolchain. The issue is solved by using custom runs.

---

## Build log

_Working notes kept during the build — raw material for sections 1–3 above.
Delete this whole section before submission._

Each entry: what the AI-drafted plan said, where I departed from it, and how the
result was checked. "The plan" = the `prompt.md` produced with Claude + ChatGPT.

### Candidate decisions for section 2 (pick the three strongest)

1. **No `shared` crate.** The plan allowed one; there was nothing real to share
   (the only cross-binary types are a small PSP request/response pair), so it
   stays two flat crates.
2. **`TEXT + CHECK` instead of Postgres `ENUM`** for the state columns. A CHECK
   constraint is a one-line migration to change later; adding `ENUM` values and
   their ordering is a known footgun.
3. **Cross-tenant safety enforced in the schema**, not by convention. A composite
   foreign key `(customer_id, business_id)` makes an invoice that points at
   another tenant's customer impossible to insert — rather than relying on every
   query remembering `WHERE business_id`.
4. **SHA-256 for API keys, not Argon2/bcrypt.** The secret is 256 bits of CSPRNG
   output, so a slow KDF only adds latency per request and defends against
   low-entropy guessing that cannot happen here.
5. **Unchecked SQL (`sqlx::query`) instead of the compile-time-checked macros.**
   See Commit 3 note — keeps every build and the Docker image database-free.

### Commit 1 — scaffold  (`990130b`)

- **Plan vs. reality:** the plan told me to add `rust-toolchain.toml` pinning
  `1.98.0`. On this Windows machine that resolves to the MSVC host toolchain,
  while the toolchain that actually works here is GNU — so `cargo build` failed
  at the linker.
- **Fix (Commit 2):** removed the file. Cargo now uses the machine default (GNU);
  the Rust version is written in the README, and the Docker build will pin it via
  the `rust:1.98` base image.
- **Verified:** workspace builds, `cargo test` green, under
  `stable-x86_64-pc-windows-gnu`.

### Commit 2 — database schema  (`da429b7`)

- **Constraint I hit:** no Docker on this machine and the system Postgres
  superuser password was lost.
- **Workaround:** verified the migration against a throwaway PG18 cluster
  (`initdb` into a temp dir, trust auth, own port). Later replaced with a
  persistent dev cluster (`scripts/pg-dev.sh`).
- **Verified:** all 9 tables and indexes create; the composite FK rejects an
  invoice referencing another tenant's customer; the partial unique index rejects
  a second `pending` payment attempt on one invoice.

### Commit 3 — config, errors, health, bootstrap  (`5df50a8`)

- **Plan vs. reality:** the plan wanted compile-time-checked queries
  (`sqlx::query!`) from the start. Those need a live database (or a committed
  `.sqlx` cache) at *build* time, which would make even the unit tests require a
  database.
- **Choice:** use unchecked `sqlx::query` for now. Every `cargo build` / `test`
  stays database-free, and the Docker image needs no `.sqlx` bundle. The SQL is
  simple and will be exercised by the integration tests in Commit 10.
- **Verified manually:** migrations run on startup; with Postgres stopped,
  `/healthz` stays `200` and `/readyz` returns `503`.

### Commit 4 — API key authentication  (`6e2795c`)

- **Plan vs. reality:** the plan specified base62 key material with fixed
  character counts.
- **Choice:** hex instead. No big-integer encoder, no extra dependency, and hex
  never contains `_`, so splitting `dodo_<id>_<secret>` needs no escaping. Same
  entropy (96-bit id, 256-bit secret).
- This is where the section-2 "SHA-256, not a KDF" decision lands in code.
- **Verified:** `invoice-service seed` against a real Postgres — two runs create
  two businesses, `secret_hash` is 32 bytes, `revoked_at` is null.

### Commit 5 — customers

- **Plan vs. reality:** the plan set Commit 5 as the point to switch to
  compile-time-checked queries with a committed `.sqlx` cache. Kept unchecked
  `sqlx::query_as` + `FromRow` for the same reason as Commit 3 — DB-free builds,
  no `.sqlx` to keep in sync — and will rely on the Commit 10 integration tests
  to exercise the SQL against a real database.
- **Dependency added:** `base64` (tiny, single purpose) for opaque pagination
  cursors, rather than hand-rolling base64 or leaking `(timestamp, id)` in the
  URL.
- **Verified with curl** against the dev cluster: auth rejection, create,
  validation envelope, get / 404, and two-page keyset pagination with a
  round-tripped cursor.

### Commit 6 — invoices and state machine

- Followed the plan closely here. The state machine is enforced by a single
  conditional `UPDATE` (`WHERE state = ANY($from)`); AI's earlier drafts had
  floated a DB trigger and `SERIALIZABLE` — both rejected as hidden control flow
  / retry-storm risk (this is section-2 material if a fourth decision is wanted).
- Chose `#[serde(deny_unknown_fields)]` to reject a client `total`, which means
  those rejections use axum's default body rather than our error envelope —
  noted as a rough edge rather than adding a custom extractor.
- **Verified with curl:** server-computed total, all validation paths, get with
  line items, list by state, void / re-void 409, and the `invoice.created`
  outbox row written in the same transaction.

### Commit 7 — mock PSP

- Straight implementation of the plan's spec table. The one judgement call:
  `tok_network_error` is a deterministic always-500 rather than "500 or a socket
  drop, alternating" — reproducible failures matter more than exercising the
  raw-socket path, and `tok_timeout` already gives the ambiguous shape.
- **Verified with curl:** every token's shape/status, idempotent replay returning
  the same `psp_ref` with no re-delay (including `tok_timeout`), and
  `/_debug/charges` omitting the `tok_network_error` key.

### Commit 8 — payment attempts + reconciliation sweeper

- **Gap in the plan I had to close:** the plan stored only
  `request_fingerprint`, never the card token. But the sweeper re-submits
  `/charge` and needs the token whenever the *first* call didn't reach the PSP.
  Added a `card_token` column (migration `0002`); kept `request_fingerprint` for
  the "same key, different body" check.
- **Bug found while testing, not from AI review:** the mock ran `tok_timeout`'s
  sleep inside the request handler, so a client disconnect cancelled it and
  nothing was stored — the sweeper then retried forever. Fixed by moving the
  delay + store into a detached task, which is also how a real processor
  behaves. This is the "one thing to correct" for section 3.
- Verified all of (a)–(e) by hand plus the sweeper recovery and the
  give-up-after-max-age path.

### Commit 9 — webhooks

- Followed the plan's claim/lease design directly. One addition of my own: a
  `WEBHOOK_ALLOW_PRIVATE_TARGETS` flag — the plan's SSRF guard would otherwise
  make webhook delivery untestable locally and broken under `docker compose`
  (sibling services are on private IPs).
- Also pulled the two duplicated random-hex helpers (API keys, webhook secrets)
  into one `secret::hex(n)`.
- **Verified** by pointing deliveries at a Python receiver that recomputes the
  HMAC — signatures matched for both event types. SSRF blocks and the retry
  backoff checked separately.

### Commit 10 — integration tests

- Wrote the three required tests plus `concurrent_timeout`. Chose to run
  everything in-process (`#[sqlx::test]` pool → `AppState` directly, mock PSP as
  an embedded router) rather than shelling out to binaries — no port juggling,
  no orphan processes.
- Made `mock-psp` lib + bin and injected timings via a `Timings` struct so the
  suite runs in ~1 minute without faking time.
- All 20 unit + 5 integration tests green under `cargo test --workspace`.
