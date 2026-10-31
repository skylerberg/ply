#!/usr/bin/env bash
# Checks that the endpoints of `examples/service.ply` behave the same against its in-memory fake
# store and against postgres.
#
# The script runs three checks: 1. `ply test examples/service.ply --no-cache` runs the whole suite
# against the fake store. `--no-cache` makes every test run again instead of reusing an earlier
# result. 2. It starts the service twice, once with `SERVICE_STORE=memory` and once with
# `SERVICE_STORE=postgres`. It sends the same requests to both and compares the responses byte for
# byte. 3. It queries postgres to check the transactional route. An order that commits must leave a
# row in the orders table. An order that is rejected after its row was inserted must leave no row.
#
# Usage: examples/same-tests.sh                              # starts a postgres server of its own
# examples/same-tests.sh --db postgres://localhost/x  # uses the database you name
# examples/same-tests.sh --db ... --reset             # drops the service's tables first
# PLY_BIN=target/release/ply examples/same-tests.sh   # runs the `ply` binary you name
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"

db=""
keep=0
reset=0
mem_port="${PLY_MEM_PORT:-8231}"
pg_port="${PLY_PG_PORT:-8232}"

while [ $# -gt 0 ]; do
  case "$1" in
    --db) db="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    --reset) reset=1; shift ;;
    -h|--help) sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

for tool in psql curl; do
  command -v "$tool" >/dev/null || { echo "$tool is needed and is not on PATH" >&2; exit 2; }
done

if [ -n "${PLY_BIN:-}" ]; then
  ply="$PLY_BIN"
else
  ply=$("$root/scripts/bootstrap.sh" --release)
fi

work="$(mktemp -d)"
owned_cluster=0
cleanup() {
  [ -n "${mem_pid:-}" ] && kill "$mem_pid" 2>/dev/null || true
  [ -n "${pg_pid:-}" ] && kill "$pg_pid" 2>/dev/null || true
  if [ "$owned_cluster" -eq 1 ] && [ "$keep" -eq 0 ]; then
    pg_ctl -D "$work/pgdata" -m immediate stop >/dev/null 2>&1 || true
  fi
  if [ "$keep" -eq 0 ]; then rm -rf "$work"; else echo "kept: $work"; fi
}
trap cleanup EXIT

echo "== 1. the whole suite against the fake, hermetically =="
"$ply" --color never test "$here/service.ply" --no-cache | tee "$work/step1.out"
counts="$(grep -E '^[[:space:]]*[0-9]+ failed, [0-9]+ passed, [0-9]+ cached' "$work/step1.out" | tail -n 1 || true)"
if [ -z "$counts" ]; then
  echo "step 1 printed no summary line to read its counts from" >&2
  exit 1
fi
passed="$(printf '%s' "$counts" | sed -E 's/^[^0-9]*[0-9]+ failed, ([0-9]+) passed.*/\1/')"
cached="$(printf '%s' "$counts" | sed -E 's/^.*, ([0-9]+) cached.*/\1/')"
if [ "$cached" -ne 0 ] || [ "$passed" -lt 1 ]; then
  echo "step 1 evaluated $passed test(s) and served $cached from a cache: '$counts'" >&2
  exit 1
fi
echo

if [ -z "$db" ]; then
  command -v initdb >/dev/null || { echo "no --db and no initdb on PATH" >&2; exit 2; }
  cluster_port="${PLY_PG_CLUSTER_PORT:-55433}"
  sock="$(mktemp -d /tmp/plypg.XXXXXX)"
  initdb -D "$work/pgdata" -U ply --locale=C --encoding=UTF8 -A trust >/dev/null
  pg_ctl -D "$work/pgdata" -l "$work/pg.log" \
    -o "-p $cluster_port -k $sock -c listen_addresses=127.0.0.1" start >/dev/null
  owned_cluster=1
  psql -h 127.0.0.1 -p "$cluster_port" -U ply -d postgres -q -c 'create database service'
  db="postgres://ply@127.0.0.1:$cluster_port/service"
  reset=1
  echo "started a postgres for this run: $db"
fi

echo "== 2. the schema, from examples/service.sql =="
if psql -tA -d "$db" -c "select to_regclass('public.items')" | grep -q items; then
  if [ "$reset" -eq 1 ]; then
    psql -v ON_ERROR_STOP=1 -q -d "$db" \
      -c 'drop table if exists "orders"' \
      -c 'drop table if exists "items"' \
      -c 'drop sequence if exists "orders_id_seq"'
  else
    echo "   $db already holds the service's tables; pass --reset, or name an empty database" >&2
    exit 2
  fi
fi
psql -v ON_ERROR_STOP=1 -q -d "$db" -f "$here/service.sql"
echo

export SERVICE_API_KEY="${SERVICE_API_KEY:-same-tests-key}"

serve() {
  local store="$1" port="$2" log="$3"
  local settings=(--config-schema service.config --set "SERVICE_STORE=$store" --set "SERVICE_PORT=$port"
    --set SERVICE_CONNECTIONS=400 --set "SERVICE_API_KEY=$SERVICE_API_KEY")
  if [ "$store" = postgres ]; then settings+=(--set "SERVICE_DATABASE=$db"); fi
  "$ply" run "$here/service.ply" "${settings[@]}" >"$log" 2>&1 &
  echo $!
}

wait_for() {
  local port="$1" tries=0
  until curl -sS -o /dev/null "http://127.0.0.1:$port/health" 2>/dev/null; do
    tries=$((tries + 1))
    [ "$tries" -gt 240 ] && return 1
    sleep 0.5
  done
}

mem_pid="$(serve memory "$mem_port" "$work/memory.log")"
pg_pid="$(serve postgres "$pg_port" "$work/postgres.log")"
wait_for "$mem_port" || { echo "the fake never answered:"; cat "$work/memory.log"; exit 1; }
wait_for "$pg_port" || { echo "postgres never answered:"; cat "$work/postgres.log"; exit 1; }

ask() {
  local method="$1" path="$2" body="${3:-}" port out
  for port in "$mem_port" "$pg_port"; do
    if [ -n "$body" ]; then
      out="$(curl -sS -m 20 -w '\n%{http_code}' -X "$method" \
        -H 'content-type: application/json' -H "x-api-key: $SERVICE_API_KEY" --data "$body" \
        "http://127.0.0.1:$port$path" 2>&1 || true)"
    else
      out="$(curl -sS -m 20 -w '\n%{http_code}' -X "$method" "http://127.0.0.1:$port$path" 2>&1 || true)"
    fi
    printf '%s' "$out" >"$work/$port.out"
  done
  if ! diff -u "$work/$mem_port.out" "$work/$pg_port.out" >"$work/diff.out"; then
    echo "   DIVERGED  $method $path"
    cat "$work/diff.out"
    divergences=$((divergences + 1))
  else
    printf '   agreed    %-42s %s\n' "$method $path" "$(tail -n1 "$work/$mem_port.out")"
  fi
  compared=$((compared + 1))
}

echo "== 3. the same requests to both, compared byte for byte =="
compared=0
divergences=0

ask GET /health
ask GET /ready
ask GET /docs/orders/placing
ask GET /docs/nowhere
ask GET /items
ask GET /items/featured
ask GET /items/gasket
ask GET /items/sprocket
ask GET "/items/';%20drop%20table%20items;%20--"
ask GET /orders
ask GET /orders/1
ask GET /orders/99
ask GET /orders/seven
ask GET /orders/1/receipt
ask POST /orders '{"customer":"hedy","lines":[{"sku":"widget","qty":2}]}'
ask POST /orders '{"customer":"ada","lines":[{"sku":"widget","qty":9}]}'
ask POST /orders '{"customer":"ada","lines":[{"sku":"widget","qty":2},{"sku":"widget","qty":2}]}'
ask POST /orders '{"customer":"ada","lines":[{"sku":"sprocket","qty":1}]}'
ask POST /orders '{"customer":"ada","lines":[]}'
ask POST /orders '{"customer":"ada","lines":[{"sku":"bolt","qty":"two"}]}'
ask GET /items
ask GET /orders
ask DELETE /orders/1
ask DELETE /orders/1
ask DELETE /orders/99
ask GET /items
ask GET /orders
ask PUT /orders
ask GET /nowhere
echo

echo "== 4. the transaction, in the database rather than in the response =="
count_orders() { psql -tA -d "$db" -c 'select count(*) from "orders"'; }
place() {
  curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' \
    -H "x-api-key: $SERVICE_API_KEY" --data "$1" "http://127.0.0.1:$pg_port/orders"
}

before="$(count_orders)"
status="$(place '{"customer":"lise","lines":[{"sku":"bolt","qty":3}]}')"
[ "$status" = 201 ] || { echo "   a placement that fits answered $status"; exit 1; }
[ "$(count_orders)" = "$((before + 1))" ] || { echo "   a committed placement left no row"; exit 1; }
echo "   committed    201, orders $before -> $((before + 1))"

before="$(count_orders)"
status="$(place '{"customer":"lise","lines":[{"sku":"widget","qty":99}]}')"
[ "$status" = 409 ] || { echo "   an over-order answered $status"; exit 1; }
[ "$(count_orders)" = "$before" ] || { echo "   a rolled-back placement left a row"; exit 1; }
echo "   rolled back  409, orders still $before"
echo

if [ "$divergences" -ne 0 ]; then
  echo "$divergences of $compared requests diverged between the fake and postgres"
  exit 1
fi
echo "$compared requests answered byte for byte alike by the fake and postgres."
