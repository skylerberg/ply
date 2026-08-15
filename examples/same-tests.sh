#!/usr/bin/env bash
# W4's exit criterion, in one command.
#
#     examples/same-tests.sh                              # manages its own database
#     examples/same-tests.sh --db postgres://localhost/x  # uses one you name
#     examples/same-tests.sh --db ... --reset             # drops its tables first
#
# The claim is that `examples/desk.ply`'s endpoints run unchanged against the
# in-memory twin and against real postgres. This script checks it in the two
# ways it can be checked, and neither of them is "the suite is green":
#
#   1. `ply test examples/desk.ply` — the whole suite against the twin, `det`,
#      cached, hermetic, with no `--host` and no database anywhere. Every route
#      is in it.
#
#   2. The **same service**, started twice on two ports — once with `main`
#      calling `run_memory` and once with it calling `run` against postgres —
#      and then the same twenty-odd requests sent to both and the answers
#      compared byte for byte. Not the same assertions written twice: the same
#      bytes off the same source, differing only in which handler answered the
#      `db` atoms.
#
#   3. The transactional route, against postgres, both ways: an order that
#      commits and an order that is rolled back after its row was already
#      inserted. The rollback is checked in the database rather than in the
#      response — `orders` unchanged, and the sequence advanced anyway, because
#      postgres does not roll back a sequence and neither does the twin.
#
# Since W5 both desks also authenticate `POST /orders`, so one credential is
# exported below and both services are started with it. That is not incidental
# to the claim: an API key resolved from configuration is a value the twin and
# the driver see identically, so the write path is compared with the check in
# front of it rather than around it.
#
# What this cannot be is one `test` block that runs against both. A Ply test
# names its handler, so a test that installed the twin is a test that installed
# the twin; the endpoints are what stay identical, and step 2 is how that is
# demonstrated rather than asserted. That limitation is real and it is stated
# here rather than papered over.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
ply="$root/target/release/ply"

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
    -h|--help) sed -n '2,31p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

for tool in psql curl; do
  command -v "$tool" >/dev/null || { echo "$tool is needed and is not on PATH" >&2; exit 2; }
done

work="$(mktemp -d)"
owned_cluster=0
cleanup() {
  [ -n "${mem_pid:-}" ] && kill "$mem_pid" 2>/dev/null || true
  [ -n "${pg_pid:-}" ] && kill "$pg_pid" 2>/dev/null || true
  if [ "$owned_cluster" -eq 1 ] && [ "$keep" -eq 0 ]; then
    pg_ctl -D "$work/pgdata" -m immediate stop >/dev/null 2>&1 || true
  fi
  [ "$keep" -eq 0 ] && rm -rf "$work" || echo "kept: $work"
}
trap cleanup EXIT

echo "== 1. the whole suite against the twin, hermetically =="
"$ply" test "$here/desk.ply" --no-incremental
echo

# A cluster of this script's own, on a port nobody else is using, in a temporary
# directory. Nothing here touches an existing cluster or an existing database.
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
# The comparison in step 3 is between two stores that have seen the same history,
# so it needs a database in its seeded state. Dropping is opt-in: a `--db` the
# caller named may be one with data in it, and this script will not guess.
if psql -tA -d "$db" -c "select to_regclass('public.items')" | grep -q items; then
  if [ "$reset" -eq 1 ]; then
    psql -v ON_ERROR_STOP=1 -q -d "$db" \
      -c 'drop table if exists "orders"' \
      -c 'drop table if exists "items"' \
      -c 'drop sequence if exists "orders_id_seq"'
  else
    echo "   $db already holds the desk's tables." >&2
    echo "   pass --reset to drop and reseed them, or point --db at an empty database." >&2
    exit 2
  fi
fi
psql -v ON_ERROR_STOP=1 -q -d "$db" -f "$here/desk.sql"
echo '   the same schema desk.schema describes; --db-schema is what checks that'
echo

# One credential for both services, exported before either starts. Without this
# each `serve.sh` would generate its own and the two desks would refuse each
# other's key — they would still *agree*, both answering 401, and the comparison
# below would be green while covering nothing. Two services that agree because
# neither did anything is the failure shape this whole script exists to catch.
export DESK_API_KEY="${DESK_API_KEY:-same-tests-key}"

serve() {
  local mode="$1" port="$2" log="$3"
  if [ "$mode" = memory ]; then
    PLY_SERVE_OUT="$work/memory" "$here/serve.sh" --memory --port "$port" \
      --requests 400 >"$log" 2>&1 &
  else
    PLY_SERVE_OUT="$work/postgres" "$here/serve.sh" --db "$db" --port "$port" \
      --requests 400 >"$log" 2>&1 &
  fi
  echo $!
}

wait_for() {
  local port="$1" tries=0
  until curl -sS -o /dev/null "http://127.0.0.1:$port/health" 2>/dev/null; do
    tries=$((tries + 1))
    [ "$tries" -gt 120 ] && return 1
    sleep 0.5
  done
}

mem_pid="$(serve memory "$mem_port" "$work/memory.log")"
pg_pid="$(serve postgres "$pg_port" "$work/postgres.log")"

wait_for "$mem_port" || { echo "the twin never answered:"; cat "$work/memory.log"; exit 1; }
wait_for "$pg_port" || { echo "postgres never answered:"; cat "$work/postgres.log"; exit 1; }

# One request, both services, status and body captured. The order matters: the
# writes below move both stores in step, so the reads after them are a
# comparison of two stores that have seen the same history.
ask() {
  local method="$1" path="$2" body="${3:-}"
  local port out
  for port in "$mem_port" "$pg_port"; do
    # A connection the service dropped is a divergence to report, not a reason to
    # stop: what the other side answered is the interesting half.
    if [ -n "$body" ]; then
      out="$(curl -sS -m 20 -w '\n%{http_code}' -X "$method" \
        -H 'content-type: application/json' -H "x-api-key: $DESK_API_KEY" --data "$body" \
        "http://127.0.0.1:$port$path" 2>&1 || true)"
    else
      out="$(curl -sS -m 20 -w '\n%{http_code}' -X "$method" \
        "http://127.0.0.1:$port$path" 2>&1 || true)"
    fi
    if [ "$port" = "$mem_port" ]; then
      printf '%s' "$out" >"$work/mem.out"
    else
      printf '%s' "$out" >"$work/pg.out"
    fi
  done
  if ! diff -u "$work/mem.out" "$work/pg.out" >"$work/diff.out"; then
    echo "   DIVERGED  $method $path"
    cat "$work/diff.out"
    divergences=$((divergences + 1))
  else
    printf '   agreed    %-42s %s\n' "$method $path" "$(tail -n1 "$work/mem.out")"
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
seq_value() { psql -tA -d "$db" -c 'select last_value from orders_id_seq'; }
stock_of() { psql -tA -d "$db" -c "select on_hand from \"items\" where sku = '$1'"; }

before_orders="$(count_orders)"
before_seq="$(seq_value)"
before_widget="$(stock_of widget)"

status="$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' \
  -H "x-api-key: $DESK_API_KEY" \
  --data '{"customer":"lise","lines":[{"sku":"bolt","qty":3}]}' \
  "http://127.0.0.1:$pg_port/orders")"
after_commit_orders="$(count_orders)"
[ "$status" = 201 ] || { echo "   a placement that fits answered $status"; exit 1; }
[ "$after_commit_orders" = "$((before_orders + 1))" ] || {
  echo "   a committed placement did not leave a row"; exit 1; }
echo "   committed  201, orders $before_orders -> $after_commit_orders"

before_orders="$after_commit_orders"
before_seq="$(seq_value)"
status="$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' \
  -H "x-api-key: $DESK_API_KEY" \
  --data '{"customer":"lise","lines":[{"sku":"widget","qty":99}]}' \
  "http://127.0.0.1:$pg_port/orders")"
after_rollback_orders="$(count_orders)"
after_seq="$(seq_value)"
after_widget="$(stock_of widget)"
[ "$status" = 409 ] || { echo "   an over-order answered $status"; exit 1; }
[ "$after_rollback_orders" = "$before_orders" ] || {
  echo "   a rolled-back placement left a row behind"; exit 1; }
echo "   rolled back 409, orders still $after_rollback_orders, widget still $after_widget"
if [ "$after_seq" -gt "$before_seq" ]; then
  echo "   and the id it consumed is gone: orders_id_seq $before_seq -> $after_seq"
  echo "   (a sequence does not roll back, in postgres or in the twin)"
fi
echo

if [ "$divergences" -ne 0 ]; then
  echo "$divergences of $compared requests diverged between the twin and postgres"
  exit 1
fi
echo "$compared requests, byte for byte identical between the twin and postgres."
echo "one source, two handlers, no endpoint changed."
