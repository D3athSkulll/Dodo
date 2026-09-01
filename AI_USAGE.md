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

## Build notes (raw — for me to fold into the sections above, not final)

Delete before submission.

- **Commit 1:** AI added `rust-toolchain.toml` pinning `1.98.0`. On this machine
  that resolved to the MSVC host toolchain (default host) while the working one
  is GNU, so `cargo build` failed on the linker. **Commit 2:** removed the file —
  cargo now uses the machine default; Rust version is documented in the README
  and pinned for Docker via the `rust:1.98` image. Verified the workspace builds
  and tests pass under `stable-x86_64-pc-windows-gnu`.
- **Commit 2:** no Docker on the box and the local Postgres superuser password
  was unknown, so the migration was verified against a throwaway PG18 cluster
  (`initdb` in a temp dir, trust auth, port 5433). Confirmed all 9 tables + all
  indexes create, the cross-tenant FK rejects a mismatched customer, and the
  partial unique index rejects a 2nd pending payment attempt.
- Candidate decisions so far (for section 2): (1) no `shared` crate; (2) `TEXT +
  CHECK` over PG `ENUM` for state columns; (3) DB-level cross-tenant integrity
  (composite FK) rather than trusting `WHERE business_id` everywhere.
- **Commit 3:** chose an unchecked `sqlx::query` for the `/readyz` `SELECT 1` so
  the build needs no database yet. AI's plan wanted compile-time-checked queries
  from the start; deferring that (with a committed `.sqlx` cache) to the first
  real queries in Commit 5 keeps every build DB-free until then. Verified the
  service manually: migrations run on startup, `/healthz` stays up with Postgres
  down, `/readyz` flips to 503.
- **Commit 4:** AI's plan specified base62 key material (fixed char counts). Used
  hex instead — no bignum encoder, no extra dependency, and hex has no `_` so the
  `dodo_<id>_<secret>` split needs no escaping. Same entropy (96 / 256 bits).
  Confirms section-2 item: SHA-256 over Argon2 for a 256-bit random secret.
  Verified `seed` against a real Postgres.
