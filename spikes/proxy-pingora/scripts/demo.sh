#!/usr/bin/env bash
# End-to-end proof for the Pingora spike: mock streaming upstream -> pingora
# proxy tap -> incremental client. Produces ../demo.log.
#
# For each provider: start the mock on 6190 serving that provider's fixture,
# send one request through the proxy (which stays up throughout), and record
# (a) client-side chunk arrival with timestamps and (b) the proxy's canonical
# event + metering log.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
MOCK=target/debug/mock_upstream
PROXY=target/debug/spike-proxy-pingora
PROXY_PORT=6188
MOCK_PORT=6190
OUT=demo.log
PROXY_LOG=$(mktemp)

cleanup() {
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null || true
  [ -n "${PROXY_PID:-}" ] && kill "$PROXY_PID" 2>/dev/null || true
}
trap cleanup EXIT

"$PROXY" --listen "127.0.0.1:$PROXY_PORT" \
  --upstream-host 127.0.0.1 --upstream-port "$MOCK_PORT" \
  --upstream-tls false >"$PROXY_LOG" 2>&1 &
PROXY_PID=$!
sleep 0.5

client() { # $1 = provider header value, $2 = request path
  python3 - "$1" "$2" <<'EOF'
import http.client, sys, time
provider, path = sys.argv[1], sys.argv[2]
conn = http.client.HTTPConnection("127.0.0.1", 6188)
conn.request("POST", path, body="{}",
             headers={"x-spike-provider": provider,
                      "Content-Type": "application/json"})
resp = conn.getresponse()
start = time.monotonic()
total = 0
chunks = 0
while True:
    chunk = resp.read1(65536)
    if not chunk:
        break
    total += len(chunk)
    chunks += 1
    print(f"[client +{time.monotonic()-start:6.3f}s] chunk {chunks}: "
          f"{len(chunk):4d}B  {chunk[:58]!r}")
print(f"[client] status={resp.status} total={total}B in {chunks} reads "
      f"(incremental: arrival spread over {time.monotonic()-start:.2f}s)")
EOF
}

run_one() { # $1 provider, $2 fixture, $3 request path
  local provider=$1 fixture=$2 path=$3
  "$MOCK" --port "$MOCK_PORT" --fixture "$fixture" --provider "$provider" \
    --delay-ms 80 2>/dev/null &
  MOCK_PID=$!
  sleep 0.3
  local mark
  mark=$(wc -l <"$PROXY_LOG")
  {
    echo "=== provider: $provider  (fixture: $fixture) ==="
    echo "--- client: incremental arrival through the proxy ---"
    client "$provider" "$path"
    sleep 0.3
    echo "--- proxy: canonical event tap + metering report ---"
    tail -n +$((mark + 1)) "$PROXY_LOG"
    echo
  } >>"$OUT"
  kill "$MOCK_PID" 2>/dev/null || true
  wait "$MOCK_PID" 2>/dev/null || true
  MOCK_PID=
}

: >"$OUT"
{
  echo "# Captured $(date -u +%Y-%m-%dT%H:%M:%SZ) by scripts/demo.sh"
  echo "# proxy: spike-proxy-pingora on 127.0.0.1:6188 -> mock upstream 127.0.0.1:6190"
  echo "# mock streams one fixture frame per 80ms over chunked transfer; the"
  echo "# client timestamps prove bytes arrive incrementally, the proxy log"
  echo "# proves the tap parsed canonical events and metered the stream."
  echo
} >>"$OUT"

run_one openai    ../event-model/fixtures/openai.sse     /v1/chat/completions
run_one anthropic ../event-model/fixtures/anthropic.sse  /v1/messages
run_one bedrock   ../event-model/fixtures/bedrock.jsonl  /model/anthropic.claude/converse-stream
# Route-prefix selection, no header: python client sends no x-spike-provider
"$MOCK" --port "$MOCK_PORT" --fixture ../event-model/fixtures/anthropic.sse \
  --provider anthropic --delay-ms 80 2>/dev/null &
MOCK_PID=$!
sleep 0.3
mark=$(wc -l <"$PROXY_LOG")
{
  echo "=== provider via route prefix (no header): /anthropic/v1/messages ==="
  echo "--- client ---"
  python3 - <<'EOF'
import http.client, time
conn = http.client.HTTPConnection("127.0.0.1", 6188)
conn.request("POST", "/anthropic/v1/messages", body="{}")
resp = conn.getresponse()
start = time.monotonic(); total = 0; chunks = 0
while True:
    chunk = resp.read1(65536)
    if not chunk: break
    total += len(chunk); chunks += 1
print(f"[client] status={resp.status} total={total}B in {chunks} reads "
      f"over {time.monotonic()-start:.2f}s")
EOF
  sleep 0.3
  echo "--- proxy ---"
  tail -n +$((mark + 1)) "$PROXY_LOG"
} >>"$OUT"

echo "wrote $OUT"
