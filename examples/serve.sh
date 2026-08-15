#!/usr/bin/env bash
# Run `examples/desk.ply` over a real socket, in plaintext or over TLS.
#
#     examples/serve.sh                     # http  on 8137
#     examples/serve.sh --tls                # https on 8443, self-signed
#     examples/serve.sh --port 9000 --requests 256
#     examples/serve.sh --tls --cert my.pem --key my.key
#
# Then, in another shell:
#
#     curl -sS localhost:8137/items
#     curl -sSk https://localhost:8443/items
#
# `desk.ply` declares no `main` — `examples/hello.ply` holds the only one under
# `examples/`, so that `ply run examples` has an unambiguous entry point. So the
# entry module is written here, into a scratch project beside the example, and
# it is six lines: the whole difference between the two transports is which of
# `run` and `run_tls` it calls.
#
# The certificate is generated rather than checked in. A private key in a
# repository is a private key that leaks, and `--tls NAME=CERT,KEY` exists so
# that the material is configured beside the run and named from the program
# rather than written into it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"

tls=0
port=""
requests=64
credential="desk"
cert=""
key=""

while [ $# -gt 0 ]; do
  case "$1" in
    --tls) tls=1; shift ;;
    --port) port="$2"; shift 2 ;;
    --requests) requests="$2"; shift 2 ;;
    --credential) credential="$2"; shift 2 ;;
    --cert) cert="$2"; shift 2 ;;
    --key) key="$2"; shift 2 ;;
    -h|--help) sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$port" ]; then
  if [ "$tls" -eq 1 ]; then port=8443; else port=8137; fi
fi

out="${PLY_SERVE_OUT:-$here/.serve}"
rm -rf "$out"
mkdir -p "$out"
cp "$here/desk.ply" "$out/desk.ply"

# `run` and `run_tls` publish `{net.write[conn], net.write[listener]}` and
# nothing else: the store and the log are discharged at the region boundary
# inside them, so this row is the whole of what the entry point can reach.
if [ "$tls" -eq 1 ]; then
  cat >"$out/main.ply" <<PLY
import std.net (net)
import desk

fn main() -> Int / {net.write[conn], net.write[listener]} =
  desk::run_tls($port, "$credential", $requests)
PLY
else
  cat >"$out/main.ply" <<PLY
import std.net (net)
import desk

fn main() -> Int / {net.write[conn], net.write[listener]} =
  desk::run($port, $requests)
PLY
fi

cargo build --release --manifest-path "$root/Cargo.toml" -p ply-cli
ply="$root/target/release/ply"

if [ "$tls" -eq 0 ]; then
  echo "serving http://localhost:$port for $requests connections"
  exec "$ply" run "$out" --host
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

# `ply hosts --host --tls ...` is the listing this run trusts: `net.listen_tls`
# on its own line, the rustls version and provider, and the credential by name
# and fingerprint. Printed before serving, because that is the moment to read it.
"$ply" hosts "$out" --host --tls "$credential=$cert,$key"

echo "serving https://localhost:$port for $requests connections (curl -k)"
exec "$ply" run "$out" --host --tls "$credential=$cert,$key"
