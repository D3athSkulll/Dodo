#!/usr/bin/env bash
# A local Postgres for development, fully separate from any system install.
#
#   Data dir : ./.pgdata   (gitignored)
#   Port     : 5433        (system Postgres usually owns 5432)
#   Auth     : trust (local only)
#
#   superuser : postgres          (no password)
#   app user  : dodo / dodo       db: dodo
#   DATABASE_URL=postgres://dodo:dodo@localhost:5433/dodo
#
# Usage:
#   scripts/pg-dev.sh init    # one-time: create the cluster, role, and db
#   scripts/pg-dev.sh start
#   scripts/pg-dev.sh stop
#   scripts/pg-dev.sh reset   # wipe and re-init
#   scripts/pg-dev.sh psql    # open a shell as dodo@dodo
#
# Point PG_BIN at your Postgres bin dir if it is not the default below.
set -euo pipefail

PORT="${PG_DEV_PORT:-5433}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PGDATA="$ROOT/.pgdata"
PG_BIN="${PG_BIN:-/c/Program Files/PostgreSQL/18/bin}"
export PATH="$PG_BIN:$PATH"

start_server() {
    pg_ctl -D "$PGDATA" -o "-p $PORT -c listen_addresses=127.0.0.1" \
        -l "$PGDATA/server.log" -w start
}

case "${1:-}" in
init)
    if [ -d "$PGDATA" ]; then
        echo ".pgdata already exists — use 'reset' to recreate"
        exit 0
    fi
    initdb -D "$PGDATA" -U postgres --auth=trust --no-locale -E UTF8 >/dev/null
    start_server
    psql -h 127.0.0.1 -p "$PORT" -U postgres -c "create role dodo login password 'dodo'"
    psql -h 127.0.0.1 -p "$PORT" -U postgres -c "create database dodo owner dodo"
    echo "ready: postgres://dodo:dodo@localhost:$PORT/dodo"
    ;;
start)
    start_server
    ;;
stop)
    pg_ctl -D "$PGDATA" -m fast -w stop
    ;;
reset)
    pg_ctl -D "$PGDATA" -m immediate -w stop 2>/dev/null || true
    rm -rf "$PGDATA"
    "$0" init
    ;;
psql)
    shift
    psql -h 127.0.0.1 -p "$PORT" -U "${PGUSER:-dodo}" -d "${PGDATABASE:-dodo}" "$@"
    ;;
*)
    echo "usage: $0 {init|start|stop|reset|psql}"
    exit 1
    ;;
esac
