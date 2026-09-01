#!/usr/bin/env bash
# Apply every migration in order with plain psql. Handy for verifying the schema
# before the app wires up sqlx::migrate!() (Commit 3), which then does this on
# startup. Linux and Git-Bash both fine.
#
#   DATABASE_URL=postgres://dodo:dodo@localhost:5432/dodo ./scripts/migrate.sh
set -euo pipefail

DB="${DATABASE_URL:?set DATABASE_URL}"
MIG_DIR="$(dirname "$0")/../crates/invoice-service/migrations"

for f in "$MIG_DIR"/*.sql; do
    echo ">> $f"
    psql "$DB" -v ON_ERROR_STOP=1 -f "$f"
done
echo "migrations applied"
