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
#     curl -sS localhost:8137/items
#     curl -sS localhost:8137/orders -d '{"customer":"ada","lines":[{"sku":"bolt","qty":4}]}'
#     curl -sS --raw localhost:8137/orders/1/receipt
#     curl -sSk https://localhost:8443/items
#
# `desk.ply` declares its own `main`, so there is no entry module to write and
# nothing here generates Ply source. What this script does instead is rewrite
# three one-line definitions in a copy — the port, the connection count, and
# which of `run`, `run_tls` and `run_memory` `main` calls — because `ply run`
# passes no arguments and a service's bounds belong in the program rather than
# in an environment variable. Every rewrite asserts it found what it replaced: a
# silent miss would serve a program this script guessed at.
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
rewrite "$out/desk.ply" "fn port() -> Int = 8137" "fn port() -> Int = $port"
rewrite "$out/desk.ply" "fn connections() -> Int = 64" "fn connections() -> Int = $requests"

if [ "$memory" -eq 1 ]; then
  rewrite "$out/desk.ply" \
    "fn main() -> Int / {Store, net.write[conn], net.write[listener]} =" \
    "fn main() -> Int / {net.write[conn], net.write[listener]} ="
  rewrite "$out/desk.ply" "    run(port(), connections())" \
    "    run_memory(port(), connections())"
elif [ "$tls" -eq 1 ]; then
  rewrite "$out/desk.ply" "    run(port(), connections())" \
    "    run_tls(port(), \"$credential\", connections())"
fi

cargo build --release --manifest-path "$root/Cargo.toml" -p ply-cli
ply="$root/target/release/ply"

if [ "$memory" -eq 1 ]; then
  # No `--db`, and none is needed: `run_memory` discharges every `db` atom
  # against a `MemDb` in a region-scoped cell, so its published row is two `net`
  # atoms and the only host handler this run binds is the socket.
  echo "serving http://localhost:$port for $requests connections (in-memory store)"
  exec "$ply" run "$out/desk.ply" --host
fi

if [ "$tls" -eq 0 ]; then
  "$ply" hosts "$out/desk.ply" --host --db "$db" --db-schema desk.schema
  echo "serving http://localhost:$port for $requests connections"
  exec "$ply" run "$out/desk.ply" --host --db "$db" --db-schema desk.schema
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
# block with the server version, the collation and the pool, and the SQL scanner
# named as what it is — a parser inside the trusted computing base. Printed
# before serving, because that is the moment to read it.
"$ply" hosts "$out/desk.ply" --host --tls "$credential=$cert,$key" \
  --db "$db" --db-schema desk.schema

echo "serving https://localhost:$port for $requests connections (curl -k)"
exec "$ply" run "$out/desk.ply" --host --tls "$credential=$cert,$key" \
  --db "$db" --db-schema desk.schema
