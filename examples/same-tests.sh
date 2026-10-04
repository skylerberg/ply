#!/usr/bin/env bash
# `examples/desk.ply`'s endpoints run unchanged against its in-memory twin and against postgres,
# checked the two ways that can be checked:
#
#   1. `ply test examples/desk.ply --no-cache`: the whole suite against the twin, hermetic, and
#      every test evaluated rather than served from a cache.
#   2. The one desk started twice, `DESK_STORE=memory` and `DESK_STORE=postgres`, the same requests
#      sent to both, and the answers compared byte for byte.
#   3. The transactional route against postgres, checked in the database: an order that commits
#      leaves a row, and one refused after its row was inserted leaves none.
#
#     examples/same-tests.sh                              # starts a postgres of its own
#     examples/same-tests.sh --db postgres://localhost/x  # uses the one you name
#     examples/same-tests.sh --db ... --reset             # drops its tables first
#     PLY_BIN=target/release/ply examples/same-tests.sh   # measures the binary you name
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
  cargo build --locked --release --quiet --manifest-path "$root/Cargo.toml" -p ply-launcher --bins
  (cd "$root" && target/release/ply-pack target/release/ply)
  ply="$root/target/release/ply"
fi
if ! "$root/.github/binary-is-current.sh" "$ply"; then
  echo "$ply is not built from this tree" >&2
  exit 2
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

echo "== 1. the whole suite against the twin, hermetically =="
"$ply" --color never test "$here/desk.ply" --no-cache | tee "$work/step1.out"
# `ply test` exits 0 over a run that evaluated nothing, so the counts are read, not the status.
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

# A cluster of the script's own, on a port and in a directory nothing else uses.
if [ -z "$db" ]; then
  command -v initdb >/dev/null || { echo "no --db and no initdb on PATH" >&2; exit 2; }
  cluster_port="${PLY_PG_CLUSTER_PORT:-55433}"
  sock="$(mktemp -d /tmp/plypg.XXXXXX)"
  initdb -D "$work/pgdata" -U ply --locale=C --encoding=UTF8 -A trust >/dev/null
  pg_ctl -D "$work/pgdata" -l "$work/pg.log" \
    -o "-p $cluster_port -k $sock -c listen_addresses=127.0.0.1" start >/dev/null
  owned_cluster=1
  psql -h 127.0.0.1 -p "$cluster_port" -U ply -d postgres -q -c 'create database desk'
  db="postgres://ply@127.0.0.1:$cluster_port/desk"
  reset=1
  echo "started a postgres for this run: $db"
fi

echo "== 2. the schema, from examples/desk.sql =="
# Both stores start from the seeded state, so a database with the tables in it is reset only when
# asked: one named with `--db` may hold data.
if psql -tA -d "$db" -c "select to_regclass('public.items')" | grep -q items; then
  if [ "$reset" -eq 1 ]; then
    psql -v ON_ERROR_STOP=1 -q -d "$db" \
      -c 'drop table if exists "orders"' \
      -c 'drop table if exists "items"' \
      -c 'drop sequence if exists "orders_id_seq"'
  else
    echo "   $db already holds the desk's tables; pass --reset, or name an empty database" >&2
    exit 2
  fi
fi
psql -v ON_ERROR_STOP=1 -q -d "$db" -f "$here/desk.sql"
echo

# One credential for both desks: two that refused each other's key would agree on every 401.
export DESK_API_KEY="${DESK_API_KEY:-same-tests-key}"

serve() {
  local store="$1" port="$2" log="$3"
  local settings=(--config-schema desk.config --set "DESK_STORE=$store" --set "DESK_PORT=$port"
    --set DESK_CONNECTIONS=400 --set "DESK_API_KEY=$DESK_API_KEY")
  if [ "$store" = postgres ]; then settings+=(--set "DESK_DATABASE=$db"); fi
  "$ply" run "$here/desk.ply" --host "${settings[@]}" >"$log" 2>&1 &
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
wait_for "$mem_port" || { echo "the twin never answered:"; cat "$work/memory.log"; exit 1; }
wait_for "$pg_port" || { echo "postgres never answered:"; cat "$work/postgres.log"; exit 1; }

# One request to both desks. The writes move both stores in step, so the reads after them compare
# two stores that have seen the same history.
ask() {
  local method="$1" path="$2" body="${3:-}" port out
  for port in "$mem_port" "$pg_port"; do
    if [ -n "$body" ]; then
      out="$(curl -sS -m 20 -w '\n%{http_code}' -X "$method" \
        -H 'content-type: application/json' -H "x-api-key: $DESK_API_KEY" --data "$body" \
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
    -H "x-api-key: $DESK_API_KEY" --data "$1" "http://127.0.0.1:$pg_port/orders"
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
  echo "$divergences of $compared requests diverged between the twin and postgres"
  exit 1
fi
echo "$compared requests answered byte for byte alike by the twin and postgres."
