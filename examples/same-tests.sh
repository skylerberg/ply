#!/usr/bin/env bash
# W4's exit criterion, in one command.
#
#     examples/same-tests.sh                              # manages its own database
#     examples/same-tests.sh --db postgres://localhost/x  # uses one you name
#     examples/same-tests.sh --db ... --reset             # drops its tables first
#     examples/same-tests.sh --no-build                   # measure a binary you built
#
# It builds `target/release/ply` itself, and checks that binary against cargo's
# own dep-info for it whether it built it or not. It used to do neither, and
# the entry this replaces in CONTRIBUTING.md §"Things known to be broken" said
# so:
#
#   > **`examples/same-tests.sh` does not build the binary it runs.** It uses
#   > `target/release/ply` (line 44) with no `cargo build` anywhere.
#
# So on a fresh clone it died at step 1 with "No such file or directory", and
# on a tree where somebody had built once it measured whatever binary happened
# to be lying there. the self-hosting spike is what that costs: a whole record's
# measurements taken against a binary built 54 seconds after an unattributed
# source edit. `--no-build` skips the build; it does not skip the check.
#
# The claim is that `examples/desk.ply`'s endpoints run unchanged against the
# in-memory twin and against real postgres. This script checks it in the two
# ways it can be checked, and neither of them is "the suite is green":
#
#   1. `ply test examples/desk.ply --no-cache` — the whole suite against the
#      twin, `det`, hermetic, with no `--host` and no database anywhere. Every
#      route is in it, and every route is *evaluated*: the counts on the
#      summary line are read, not just the exit status.
#
#      This step used to claim less carefully than it checked. It said:
#
#      >   1. `ply test examples/desk.ply` — the whole suite against the twin,
#      >      `det`, cached, hermetic, with no `--host` and no database
#      >      anywhere. Every route is in it.
#
#      and it passed `--no-incremental`, which disables only the *front-end*
#      cache — `crates/ply-cli/src/cli.rs:358-359` says so in as many words,
#      and `crates/ply-cli/src/commands/test.rs:50` is
#      `let incremental = !args.no_incremental && !no_cache;`. On a warm
#      `examples/.ply-cache` the step therefore printed
#      `0 failed, 0 passed, 68 cached (0.00s)` and exited 0 having evaluated
#      nothing at all. `cached` was never a property step 1 could advertise and
#      still be evidence; it was the hole the step reported success through.
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
build=1
mem_port="${PLY_MEM_PORT:-8231}"
pg_port="${PLY_PG_PORT:-8232}"

while [ $# -gt 0 ]; do
  case "$1" in
    --db) db="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    --reset) reset=1; shift ;;
    --no-build) build=0; shift ;;
    # The whole leading comment, however long it grows. A fixed line range goes
    # stale silently: `sed -n '2,31p'` was this line until the header outgrew
    # 31 lines, and `--help` then stopped mid-sentence at "a value the twin and".
    -h|--help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' \
      "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done


needed=(psql curl)
if [ "$build" -eq 1 ]; then needed+=(cargo); fi
for tool in "${needed[@]}"; do
  command -v "$tool" >/dev/null || { echo "$tool is needed and is not on PATH" >&2; exit 2; }
done

# `--locked`, and it was not before. This build re-resolves `Cargo.lock` if the
# manifests have moved past it, and re-resolving is the one thing a measuring
# script must not do quietly: in CI this runs *after* a `cargo build --locked`
# step, so an unlocked build here could update the very file that step had just
# vouched for and leave a green tick over a lock nothing checked. Measured on
# this tree with one `[[package]]` entry deleted from the lock: without
# `--locked` the build exits 0 and silently rewrites the file; with it, exit 101,
# `cannot update the lock file ... because --locked was passed`, file untouched.
# The cost is that a tree whose lock is genuinely stale must run `cargo build`
# once before measuring — the same bargain the `clippy` and `test` jobs already
# make, and a clearer failure than a lock that moved under a run.
if [ "$build" -eq 1 ]; then
  cargo build --locked --release --manifest-path "$root/Cargo.toml" -p ply-cli
fi

# ~~This script does not build the binary it runs (CONTRIBUTING.md §"Things
# known to be broken" item 2), so the one thing it can do is refuse to run
# against a binary that is not the tree.~~ **It builds now** — see the `--no-build`
# handling above, which closed item 2. This check stays, and is the stronger half:
# a build can succeed and still leave `$ply` stale if `CARGO_TARGET_DIR` points
# elsewhere, and a build cannot notice a `.ply` edit at all. `desk.ply` imports all eight `std` modules and
# they are `include_str!`ed into `ply`, so an edit to one changes what this
# comparison means and moves no `.rs` file: `find crates -name '*.rs' -newer`
# would call it clean. See CONTRIBUTING.md §"The binary is an instrument too".
if ! "$root/.github/binary-is-current.sh" "$ply"; then
  echo >&2
  echo "   $ply is not built from this tree." >&2
  echo "   cargo build --release -p ply-cli" >&2
  exit 2
fi

# The instrument, checked before it is used, on both paths. `target/release/ply.d`
# is cargo's own dep-info for this exact binary — the sources cargo actually
# compiled into it, and nothing else. That is the right domain. The wider `find
# crates -name '*.rs' -newer target/release/ply` fires on edits that cannot change
# this binary and cannot be cleared by rebuilding, and a guard that cries wolf
# gets commented out.
#
# How big that domain is, the loop below counts and prints. It used to be written
# here instead:
#
#   > is cargo's own dep-info for this exact binary: 152 files on this tree,
#   > twelve crates, and neither `ply-corpus` nor `ply-codegen-spike` nor any
#   > crate's `tests/` among them.
#
# That was true when it was written and it is still true: on 2026-08-27 this tree
# printed `152 sources across 12 crates`, and
# `sed -n '1s/^[^:]*://p' target/release/ply.d | tr ' ' '\n' |
#  grep -c 'ply-corpus\|ply-codegen-spike\|/tests/'` is 0 — but it is a figure
# that moves whenever a crate is added, with nothing to notice, and the same
# figure had been copied into CONTRIBUTING.md. Counting costs one `$((...))` per
# file and cannot go stale.
#
# The count is load-bearing rather than decoration. Cargo's dep-info is
# `target: src src ...` on line 1; a format that moved parses to **zero**
# sources, and a loop over zero sources finds no stale file and pronounces every
# binary you could hand it fresh. So `sources` must be at least 1 — a floor, like
# step 1's `passed >= 1`, never an equality against a number that would turn this
# script red the day a module is added.
#
# What it catches: a binary older than a source compiled into it, and a binary
# that is not there — including a `CARGO_TARGET_DIR` that sent the build
# somewhere `$ply` does not point, which the bare path below missed silently.
# What it does not: an edit and a build inside the same second, mtime being what
# it is. That is the second-granularity trap CONTRIBUTING.md §"A moving tree
# invalidates a correctness number" already records for cargo's own
# fingerprints, and this check inherits a weaker form of it. It is a check, not
# a guarantee.
if [ ! -x "$ply" ]; then
  echo "no release binary at $ply" >&2
  if [ "$build" -eq 0 ]; then
    echo "   --no-build was passed, so this script did not build one" >&2
  else
    echo "   the build reported success, so CARGO_TARGET_DIR may point elsewhere" >&2
  fi
  exit 2
fi
dep="$root/target/release/ply.d"
if [ ! -f "$dep" ]; then
  echo "no dep-info at $dep, so $ply cannot be checked against its own sources" >&2
  exit 2
fi
stale=""
sources=0
crates_seen=""
for src in $(sed -n '1s/^[^:]*://p' "$dep"); do
  sources=$((sources + 1))
  case "$src" in
    */crates/*) rest="${src#*/crates/}"; crates_seen="$crates_seen ${rest%%/*}" ;;
  esac
  # No `break`: the whole list is walked so that what gets printed is a count of
  # what was checked rather than of what was reached before the first failure.
  if [ -z "$stale" ] && [ -e "$src" ] && [ "$src" -nt "$ply" ]; then stale="$src"; fi
done
if [ "$sources" -eq 0 ]; then
  echo "$dep named no sources, so nothing was compared against $ply" >&2
  echo "   cargo writes 'target: src src ...' on line 1; that shape moved" >&2
  exit 2
fi
if [ -n "$stale" ]; then
  echo "$ply is older than a source it was built from:" >&2
  echo "   $stale" >&2
  echo "   rebuild it before measuring anything with it" >&2
  exit 2
fi
crates="$(printf '%s' "$crates_seen" | tr ' ' '\n' | sort -u | grep -c . || true)"
echo "instrument: $sources sources across $crates crates in ${dep#"$root"/}, none newer than the binary"
echo

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
"$ply" test "$here/desk.ply" --no-cache | tee "$work/step1.out"

# The counts, not just the exit status. `ply test` exits 0 over a run that
# evaluated nothing, which is what this step did for as long as it passed
# `--no-incremental`. The line read here is printed by `print_summary` at
# `crates/ply-cli/src/commands/test.rs:1016` as
# "{IND}{failed}, {passed}, {cached} cached{hosted} ({:.2}s)" — `IND` is three
# spaces, and `{hosted}` sits between the count and the seconds when a run is
# host-backed, which is why neither end of the line is anchored.
# `ply_test::RunReport::summary` (`crates/ply-test/src/report.rs:220`) builds
# the same shape for other consumers; nothing pins either. So a format that
# moves aborts here, loudly, rather than quietly reverting this step to
# checking an exit status — the same reason `serve.sh`'s `rewrite()` refuses
# instead of guessing.
# `tee` makes step 1's stdout a pipe, so `style.rs:32` leaves it uncoloured —
# step 1 alone prints plain, and that is the price of reading its counts.
# Escapes are stripped regardless, so a change to that detection cannot disarm
# this guard: a line it cannot parse has to mean the format moved.
esc="$(printf '\033')"
counts="$(sed "s/${esc}\[[0-9;]*m//g" "$work/step1.out" \
  | grep -E '^[[:space:]]*[0-9]+ failed, [0-9]+ passed, [0-9]+ cached' \
  | tail -n 1 || true)"
if [ -z "$counts" ]; then
  echo "step 1 printed no counts line in the shape crates/ply-cli/src/commands/test.rs:1016 builds" >&2
  echo "   so nothing here can tell you whether it checked anything; the format moved" >&2
  exit 1
fi
passed="$(printf '%s' "$counts" | sed -E 's/^[^0-9]*[0-9]+ failed, ([0-9]+) passed.*/\1/')"
cached="$(printf '%s' "$counts" | sed -E 's/^.*, ([0-9]+) cached.*/\1/')"
if [ "$cached" -ne 0 ]; then
  echo "step 1 served $cached test(s) from the result cache: '$counts'" >&2
  echo "   --no-cache is what forces the run; a cached step 1 is not evidence" >&2
  exit 1
fi
if [ "$passed" -lt 1 ]; then
  echo "step 1 evaluated no tests at all: '$counts'" >&2
  echo "   an exit status over an empty suite is the failure this check exists for" >&2
  exit 1
fi
echo "   $passed tests evaluated, 0 served from cache"
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
