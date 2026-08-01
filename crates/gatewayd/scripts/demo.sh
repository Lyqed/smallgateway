#!/usr/bin/env bash
# End-to-end proof for Phase 1, milestone 1: gatewayd serving the Baseline
# from one static config file (demo/gateway.yaml). Produces ../demo.log.
# Harness promoted from spikes/proxy-pingora/scripts/demo.sh (Phase 0).
#
# Scenarios, each with real curl output plus the gateway's own log:
#   (1) GB-1  missing attribution tag -> the operator's body and status
#   (2) GB-3  caller forges a pinned key -> gateway overwrites it (visible
#             in the x-echo-attr-* headers the mock upstream reflects back)
#   (3)       happy path: tagged request streams through with 1:1 chunk
#             cadence; the end-of-stream log line joins attribution values
#             with token counts (all three providers)
#   (4) GB-4  unknown route -> the operator's unknown_route template
#   (5) GB-2  claim-mapped key: verified JWT -> proven tag; forged caller
#             header without a token -> rejected, never believed
#   (6)       hardening: dot-segment paths (literal and %2e-encoded) are
#             resolved BEFORE route matching, so a traversal spelling can
#             never select a weaker attribution contract
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
BIN=../../target/debug
PORT=6188
MOCK_PORT=6190
OUT=demo.log
GW_LOG=$(mktemp)
FIXTURES=../../spikes/event-model/fixtures

cleanup() {
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null || true
  [ -n "${GW_PID:-}" ] && kill "$GW_PID" 2>/dev/null || true
}
trap cleanup EXIT

"$BIN/gatewayd" --config demo/gateway.yaml --listen "127.0.0.1:$PORT" \
  >"$GW_LOG" 2>&1 &
GW_PID=$!
sleep 0.6

start_mock() { # $1 = fixture, $2 = provider
  "$BIN/mock_upstream" --port "$MOCK_PORT" --fixture "$1" --provider "$2" \
    --delay-ms 80 2>/dev/null &
  MOCK_PID=$!
  sleep 0.3
}

stop_mock() {
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null || true
  wait "${MOCK_PID:-}" 2>/dev/null || true
  MOCK_PID=
}

mark() { wc -l <"$GW_LOG"; }
gw_since() { tail -n +"$(( $1 + 1 ))" "$GW_LOG"; }

# Timestamped incremental reader: proves chunks arrive one mock frame at a
# time through the proxy, not coalesced. Reads curl's stdout as it flushes.
stream_client() { # $* = curl args
  curl -sN --no-buffer "$@" | python3 -c '
import os, sys, time
start = time.monotonic(); n = 0; total = 0
while True:
    chunk = os.read(0, 65536)
    if not chunk:
        break
    n += 1; total += len(chunk)
    print(f"[client +{time.monotonic()-start:6.3f}s] read {n}: "
          f"{len(chunk):4d}B  {chunk[:58]!r}")
print(f"[client] total={total}B in {n} reads "
      f"(incremental: arrival spread over {time.monotonic()-start:.2f}s)")
'
}

mint_jwt() { # HS256 token for the demo secret; sub=alice
  python3 -c '
import base64, hashlib, hmac, json
def b64(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()
h = b64(json.dumps({"alg": "HS256", "typ": "JWT"}).encode())
p = b64(json.dumps({"sub": "alice", "exp": 4102444800}).encode())
sig = b64(hmac.new(b"demo-secret-do-not-deploy", f"{h}.{p}".encode(),
                   hashlib.sha256).digest())
print(f"{h}.{p}.{sig}")
'
}

: >"$OUT"
{
  echo "# Captured $(date -u +%Y-%m-%dT%H:%M:%SZ) by scripts/demo.sh"
  echo "# gatewayd on 127.0.0.1:$PORT, config demo/gateway.yaml,"
  echo "# mock upstream on 127.0.0.1:$MOCK_PORT streaming spike fixtures"
  echo "# (one frame per 80ms) and echoing received x-attr-* headers back"
  echo "# as x-echo-attr-* response headers."
  echo
  echo "=== (0) startup: config loaded, validated, routes bound ==="
  gw_since 0
  echo
} >>"$OUT"

# --- (1) GB-1: missing required tag -> operator body, verbatim -----------
M=$(mark)
{
  echo "=== (1) GB-1: no x-attr-team on /openai -> operator's 428, not a bare 4xx ==="
  echo "--- curl -is (no attribution headers) ---"
  curl -is "http://127.0.0.1:$PORT/openai/v1/chat/completions" \
    -H 'content-type: application/json' -d '{}'
  echo
  echo "--- gateway log ---"
  gw_since "$M"
  echo
} >>"$OUT"

# --- (2) GB-3: forged pinned key is overwritten --------------------------
start_mock "$FIXTURES/openai.sse" openai
M=$(mark)
{
  echo "=== (2) GB-3: caller sends x-attr-env: shadow-prod, but env is pinned to prod ==="
  echo "    (x-echo-attr-* headers show what the UPSTREAM actually received)"
  echo "--- curl -s -D - -o /dev/null ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/openai/v1/chat/completions" \
    -H 'x-attr-team: ml-research' -H 'x-attr-env: shadow-prod' \
    -H 'content-type: application/json' -d '{}'
  echo "--- gateway log ---"
  gw_since "$M"
  echo
} >>"$OUT"
stop_mock

# --- (3) happy path: streaming tap + attribution/spend join --------------
run_happy() { # $1 provider kind, $2 fixture, $3 request path
  start_mock "$2" "$1"
  local m
  m=$(mark)
  {
    echo "=== (3) happy path [$1]: tagged request streams through, 1:1 chunk cadence ==="
    echo "--- client: timestamped incremental reads through the gateway ---"
    stream_client "http://127.0.0.1:$PORT$3" \
      -H 'x-attr-team: ml-research' \
      -H 'content-type: application/json' -d '{}'
    sleep 0.3
    echo "--- gateway log: canonical events + the attribution->spend join line ---"
    gw_since "$m"
    echo
  } >>"$OUT"
  stop_mock
}

run_happy openai    "$FIXTURES/openai.sse"    /openai/v1/chat/completions
run_happy anthropic "$FIXTURES/anthropic.sse" /anthropic/v1/messages
run_happy bedrock   "$FIXTURES/bedrock.jsonl" /bedrock/model/anthropic.claude/converse-stream

# --- (4) GB-4: unknown route -> operator template ------------------------
M=$(mark)
{
  echo "=== (4) GB-4: unmatched path -> operator's unknown_route template ==="
  echo "--- curl -is ---"
  curl -is "http://127.0.0.1:$PORT/v2/definitely-not-a-route"
  echo
  echo "--- gateway log ---"
  gw_since "$M"
  echo
} >>"$OUT"

# --- (5) GB-2: proven claims vs forged caller header ---------------------
start_mock "$FIXTURES/openai.sse" openai
TOKEN=$(mint_jwt)
M=$(mark)
{
  echo "=== (5) GB-2: /claims maps user from the JWT sub claim ==="
  echo "--- (5a) verified token -> user=alice(proven); forged x-attr-user ignored ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/claims/v1/chat/completions" \
    -H "authorization: Bearer $TOKEN" \
    -H 'x-attr-team: ml-research' -H 'x-attr-user: mallory' \
    -H 'content-type: application/json' -d '{}'
  sleep 1.2
  echo "--- (5b) no token, forged x-attr-user only -> rejected, never believed ---"
  curl -is "http://127.0.0.1:$PORT/claims/v1/chat/completions" \
    -H 'x-attr-team: ml-research' -H 'x-attr-user: mallory' \
    -H 'content-type: application/json' -d '{}'
  echo
  echo "--- gateway log ---"
  gw_since "$M"
} >>"$OUT"
stop_mock

# --- (6) hardening: dot-segments cannot select a weaker contract ----------
M=$(mark)
{
  echo
  echo "=== (6) hardening: /openai/../claims resolves to /claims BEFORE route matching ==="
  echo "    (the traversal spelling lands on the STRONGER contract: user must be proven)"
  echo "--- curl -is --path-as-is /openai/../claims/... (forged x-attr-user, no token) ---"
  curl -is --path-as-is "http://127.0.0.1:$PORT/openai/../claims/v1/chat/completions" \
    -H 'x-attr-team: ml-research' -H 'x-attr-user: mallory' \
    -H 'content-type: application/json' -d '{}'
  echo
  echo "--- same bypass spelled with %2e%2e-encoded dots ---"
  curl -is --path-as-is "http://127.0.0.1:$PORT/openai/%2e%2e/claims/v1/chat/completions" \
    -H 'x-attr-team: ml-research' -H 'x-attr-user: mallory' \
    -H 'content-type: application/json' -d '{}'
  echo
  echo "--- gateway log ---"
  gw_since "$M"
} >>"$OUT"

echo "wrote $OUT"
