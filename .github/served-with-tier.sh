#!/usr/bin/env bash
# `examples/hello.ply` served by `ply run --host` with the chain entered whole: the tier holds the
# accept loop, so the production region and the host route answer a real connection, and the run
# exits when the example has served its count. ADR 0044's fourth stage is what this checks.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bin=${1:-$root/target/release/ply}
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
sed -e "s/^fn port() -> Int = 8080\$/fn port() -> Int = $port/" \
    -e 's/^fn connections() -> Int = 64$/fn connections() -> Int = 2/' \
    "$root/examples/hello.ply" > "$dir/hello.ply"
grep -q "^fn port() -> Int = $port\$" "$dir/hello.ply" || {
  echo "examples/hello.ply no longer declares its port as this script rewrites it"; exit 1; }

# Two connections: the probe below that waits for the listener is the first, answered as a
# peer that says nothing, and the request is the second, after which the example exits.
cd "$dir"
PLY_C_CACHE="$dir/cache" PLY_C_EMITTER="ply:$root/crates/ply-compiler" PLY_C_REFUSALS=1 \
  "$bin" run --host --backend c > out.txt 2> err.txt &
pid=$!
for _ in $(seq 1 300); do
  python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $port)) == 0 else 1)" 2>/dev/null && break
  kill -0 $pid 2>/dev/null || { echo "the server exited before listening:"; cat err.txt | tail -20; exit 1; }
  sleep 0.1
done
answer=$(python3 - "$port" <<'PY'
import socket, sys
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=10)
s.sendall(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
data = b""
while True:
    chunk = s.recv(4096)
    if not chunk:
        break
    data += chunk
print(data.decode(errors="replace"))
PY
)
set +e
wait $pid
status=$?
set -e
grep -q "hello from ply" <<<"$answer" || {
  echo "the server did not answer the request:"; echo "$answer"; tail -20 err.txt; exit 1; }
if grep -q 'c tier refused `hello\.' err.txt; then
  echo "the tier refused part of the example, so the machine served it:"; grep 'refused `hello\.' err.txt; exit 1
fi
grep -qE "c tier took [0-9]+ of [0-9]+ definitions" err.txt || { echo "no tier ran:"; tail -20 err.txt; exit 1; }
[ "$status" = 0 ] || { echo "the run exited $status:"; tail -20 err.txt; exit 1; }
echo "served: the tier held the accept loop and the run exited when its count was served"
