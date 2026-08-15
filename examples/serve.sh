#!/usr/bin/env bash
# Run `examples/desk.ply` over a real socket, against postgres or against the
# in-memory twin, in plaintext or over TLS.
#
#     examples/serve.sh --memory                       # no database at all
#     examples/serve.sh --db postgres://localhost/desk
#     examples/serve.sh --db postgres://localhost/desk --tls
#     examples/serve.sh --memory --port 9000 --requests 256
#
# Then, in another shell:
#
#     curl -sS localhost:8137/health     # the process is up            → 200
#     curl -sS localhost:8137/ready      # it should get traffic         → 200 or 503
#     curl -sS localhost:8137/items
#     curl -sS localhost:8137/orders -H "x-api-key: $DESK_API_KEY" \
#       -d '{"customer":"ada","lines":[{"sku":"bolt","qty":4}]}'
#     curl -sS --raw localhost:8137/orders/1/receipt
#     curl -sSk https://localhost:8443/items
#
# and then ^C, or `kill -TERM`: the listener stops accepting, the request in
# flight finishes, every open transaction rolls back, every open span closes
# `Abandoned`, the sink flushes, the pool closes, and the process exits 0.
#
# **The port, the connection budget and the API key are configuration** and are
# passed with `--set`, not written into the source. That is W5 paying for
# itself: this script used to rewrite two one-line definitions to change a port,
# and now it passes an argument. What it still rewrites is *one* line — which of
# `run`, `run_tls` and `run_memory` `main` calls — because which store a service
# uses is not configuration. A value that decides what the program is *specified*
# in terms of would be a value no test covers.
#
# `desk.ply` declares its own `main`, so there is no entry module to write and
# nothing here generates Ply source. The two rewrites below each assert they
# found what they replaced: a silent miss would serve a program this script
# guessed at.
#
# The database is created out of band, by `examples/desk.sql`. W4 ships a schema
# as a value and refuses to ship a migration tool, so the guarantee it offers is
# the other one — `--db-schema desk.schema` is passed on every run below, and the
# driver refuses at bind time with `E0435` if the live database is not the one
# `desk.ply` describes.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"

tls=0
memory=0
db=""
port=""
requests=64
credential="desk"
cert=""
key=""

while [ $# -gt 0 ]; do
  case "$1" in
    --memory) memory=1; shift ;;
    --db) db="$2"; shift 2 ;;
    --tls) tls=1; shift ;;
    --port) port="$2"; shift 2 ;;
    --requests) requests="$2"; shift 2 ;;
    --credential) credential="$2"; shift 2 ;;
    --cert) cert="$2"; shift 2 ;;
    --key) key="$2"; shift 2 ;;
    -h|--help) sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$memory" -eq 0 ] && [ -z "$db" ]; then
  echo "give a database with --db, or run the twin with --memory" >&2
  exit 2
fi
if [ "$memory" -eq 1 ] && [ "$tls" -eq 1 ]; then
  echo "--memory serves plaintext: run_memory is the twin's plain entry point" >&2
  exit 2
fi

if [ -z "$port" ]; then
  if [ "$tls" -eq 1 ]; then port=8443; else port=8137; fi
fi

out="${PLY_SERVE_OUT:-$here/.serve}"
rm -rf "$out"
mkdir -p "$out"

# One rewrite, refusing to guess. `to` may contain `&` and `|`, so the
# substitution is done in awk over whole lines rather than in sed.
rewrite() {
  local file="$1" from="$2" to="$3"
  if ! grep -qF -- "$from" "$file"; then
    echo "examples/desk.ply no longer contains: $from" >&2
    echo "this script rewrites it and must be updated with it" >&2
    exit 1
  fi
  awk -v from="$from" -v to="$to" '{ if (index($0, from)) { print to } else { print } }' \
    "$file" > "$file.new"
  mv "$file.new" "$file"
}

cp "$here/desk.ply" "$out/desk.ply"

# The twin's row is narrower than the real desk's — `run_memory` discharges every
# `db` and `trace` atom itself — so the entry point's annotation moves with the
# call. Both are one line, deliberately: a rewrite that has to match a wrapped
# annotation stops matching the first time somebody reflows it.
if [ "$memory" -eq 1 ]; then
  rewrite "$out/desk.ply" \
    "fn main() -> Int / {Serving, config.read[server], net.write[conn], net.write[listener]} = {" \
    "fn main() -> Int / {config.read[server], config.read[credentials], net.write[conn], net.write[listener]} = {"
  rewrite "$out/desk.ply" "    run(port, count)" "    run_memory(port, key, count)"
elif [ "$tls" -eq 1 ]; then
  rewrite "$out/desk.ply" "    run(port, count)" \
    "    run_tls(port, \"$credential\", count)"
fi

# The credential. A deployment exports one; a development run that did not gets a
# generated one, printed — because a key nobody can read is a desk nobody can
# post an order to. `--config-schema desk.config` is what makes an unset one a
# start-up refusal (`E0441`) rather than a 401 on the first order.
if [ -z "${DESK_API_KEY:-}" ]; then
  DESK_API_KEY="dev-$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  export DESK_API_KEY
  echo "DESK_API_KEY was unset; this run generated one:"
  echo "    export DESK_API_KEY=$DESK_API_KEY"
  echo "it is a development credential and is printed because it was made here."
fi

# Every knob a run may differ in, in one place. The port and the budget are
# `--set`; the trace sink and the drain window belong to the run rather than to
# the program, which is why they are flags and `limits()` is a definition.
settings=(
  --config-schema desk.config
  --set "DESK_PORT=$port"
  --set "DESK_CONNECTIONS=$requests"
  --trace json
  --trace-level info
  --drain-ms 30000
)

cargo build --release --manifest-path "$root/Cargo.toml" -p ply-cli
ply="$root/target/release/ply"

if [ "$memory" -eq 1 ]; then
  # No `--db`, and none is needed: `run_memory` discharges every `db` atom
  # against a `MemDb` in a region-scoped cell, so its published row is two `net`
  # atoms and the only host handler this run binds is the socket.
  # No trace lines will appear, and that is the twin working rather than the
  # sink failing: `run_memory` discharges every `trace` atom in Ply, so nothing
  # reaches `ply_host::trace`. Run against a database to watch the sink.
  echo "serving http://localhost:$port for $requests connections (in-memory store)"
  exec "$ply" run "$out/desk.ply" --host "${settings[@]}"
fi

if [ "$tls" -eq 0 ]; then
  "$ply" hosts "$out/desk.ply" --host --db "$db" --db-schema desk.schema "${settings[@]}"
  echo "serving http://localhost:$port for $requests connections"
  exec "$ply" run "$out/desk.ply" --host --db "$db" --db-schema desk.schema "${settings[@]}"
fi

if [ -z "$cert" ] || [ -z "$key" ]; then
  cert="$out/$credential.pem"
  key="$out/$credential.key"
  # Self-signed and short-lived, because it exists to exercise the handshake
  # and nothing else. `curl` needs `-k`, which is the honest form of "this
  # certificate proves nothing".
  openssl req -x509 -newkey rsa:2048 -sha256 -days 1 -nodes \
    -keyout "$key" -out "$cert" -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null
fi

# `ply hosts` is the listing this run trusts: the socket handlers, the database
# block with the server version, the collation and the pool, the SQL scanner
# named as what it is — a parser inside the trusted computing base — and now the
# configuration, observability and shutdown blocks, with every secret key shown
# as `****` beside the source that supplied it. Printed before serving, because
# that is the moment to read it.
"$ply" hosts "$out/desk.ply" --host --tls "$credential=$cert,$key" \
  --db "$db" --db-schema desk.schema "${settings[@]}"

echo "serving https://localhost:$port for $requests connections (curl -k)"
exec "$ply" run "$out/desk.ply" --host --tls "$credential=$cert,$key" \
  --db "$db" --db-schema desk.schema "${settings[@]}"
