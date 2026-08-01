#!/usr/bin/env bash
# End-to-end proof for Phase 1, milestones 1 + 2: gatewayd serving the
# Baseline from one config file, hot-swapped live. Produces ../demo.log.
# Harness promoted from spikes/proxy-pingora/scripts/demo.sh (Phase 0).
#
# The gateway runs against a MUTABLE COPY of demo/gateway.yaml (in a temp
# dir) so scenarios 7-9 can rewrite it; --poll-interval 2 arms the file
# watcher and SIGHUP is the immediate trigger. Every [req]/[attr]/[meter]
# log line carries cfg=vN — the version that served that request.
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
#   (7)       hot swap: SIGHUP mid-stream -> the in-flight stream drains
#             under cfg=v1 while a concurrent request runs cfg=v2; the
#             poll watcher then hash-no-ops the same change (doc 03)
#   (8)       NACK: an invalid config is REJECTED loudly by both triggers;
#             the old snapshot keeps serving
#   (9)       no-op: reloading identical content is hash-detected and
#             debug-logged, no new version
#   (10) GB-8 vertex billing labels: operator labels (static +
#             attribution-derived + CEL) merged into the generateContent
#             BODY; operator wins over the client's spoof; the mock echoes
#             the body labels it received as x-echo-label-* headers.
#             Runs under cfg=v2, so the env label reads "canary".
#   (11) GB-7 STS session tags: AssumeRole against a MOCK STS with tags
#             from resolved attribution; the mock Bedrock REQUIRES a valid
#             SigV4 signature, decodes the session tags from the security
#             token, and echoes them as x-echo-session-tag-* headers. A
#             second identical request is served from the credential cache
#             (same access key, no second AssumeRole in the mock-sts log).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
BIN=../../target/debug
PORT=6188
MOCK_PORT=6190
OUT=demo.log
GW_LOG=$(mktemp)
FIXTURES=../../spikes/event-model/fixtures

# Milestone 2: the served config is a mutable copy — the repo's demo file
# stays pristine while scenarios 7-9 swap versions underneath the gateway.
CFG_DIR=$(mktemp -d)
CFG="$CFG_DIR/gateway.yaml"
cp demo/gateway.yaml "$CFG"

cleanup() {
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null || true
  [ -n "${GW_PID:-}" ] && kill "$GW_PID" 2>/dev/null || true
  [ -n "${STS_PID:-}" ] && kill "$STS_PID" 2>/dev/null || true
  [ -n "${BEDROCK_PID:-}" ] && kill "$BEDROCK_PID" 2>/dev/null || true
  rm -rf "$CFG_DIR"
}
trap cleanup EXIT

# gatewayd=debug so the no-op reload lines (debug level by design) are
# visible; everything else stays at info.
RUST_LOG="info,gatewayd=debug" "$BIN/gatewayd" --config "$CFG" \
  --listen "127.0.0.1:$PORT" --poll-interval 2 >"$GW_LOG" 2>&1 &
GW_PID=$!
sleep 0.6

start_mock() { # $1 = fixture, $2 = provider, $3 = delay-ms (default 80)
  "$BIN/mock_upstream" --port "$MOCK_PORT" --fixture "$1" --provider "$2" \
    --delay-ms "${3:-80}" 2>/dev/null &
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
  echo "# gatewayd on 127.0.0.1:$PORT, serving a mutable copy of"
  echo "# demo/gateway.yaml (hot-swapped live in scenarios 7-9; SIGHUP +"
  echo "# a 2s poll watcher). Mock upstream on 127.0.0.1:$MOCK_PORT streams"
  echo "# spike fixtures (one frame per 80ms unless noted) and echoes"
  echo "# received x-attr-* headers back as x-echo-attr-* response headers."
  echo "# Every [req]/[attr]/[meter] line carries cfg=vN."
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

# --- (7) hot swap: the drain overlap made visible -------------------------
# A slow stream (600ms/frame, ~3.5s total) starts under cfg=v1; mid-stream
# the config is swapped to v2 (env pin prod -> canary) via SIGHUP. The old
# stream finishes and METERS under v1; a concurrent request binds v2 and
# its upstream echo shows env=canary. The poll watcher then notices the
# same mtime change and hash-no-ops it: two triggers, one reload path.
start_mock "$FIXTURES/openai.sse" openai 600
STREAM_A=$(mktemp)
M=$(mark)
{
  echo
  echo "=== (7) hot swap: SIGHUP mid-stream; v1 stream drains while v2 serves new requests ==="
  echo "    (demo/gateway.v2.yaml flips the /openai pin env=prod -> env=canary)"
} >>"$OUT"
stream_client "http://127.0.0.1:$PORT/openai/v1/chat/completions" \
  -H 'x-attr-team: ml-research' \
  -H 'content-type: application/json' -d '{}' >"$STREAM_A" 2>&1 &
A_PID=$!
sleep 1.2 # the v1 stream is mid-flight
cp demo/gateway.v2.yaml "$CFG"
kill -HUP "$GW_PID"
sleep 0.5
{
  echo "--- concurrent request WHILE the v1 stream is still in flight (binds v2) ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/openai/v1/chat/completions" \
    -H 'x-attr-team: ml-research' -H 'x-attr-env: shadow-prod' \
    -H 'content-type: application/json' -d '{}'
} >>"$OUT"
wait "$A_PID"
sleep 2.5 # let the poll watcher observe the mtime change and hash-no-op it
{
  echo "--- client A: the stream that started under v1, uninterrupted ---"
  cat "$STREAM_A"
  echo "--- gateway log: swap v1->v2, v2 request+meter, then the v1 meter line AFTER them ---"
  gw_since "$M"
  echo
} >>"$OUT"
rm -f "$STREAM_A"
stop_mock

# --- (8) NACK: invalid config rejected, old snapshot keeps serving --------
start_mock "$FIXTURES/openai.sse" openai
M=$(mark)
{
  echo "=== (8) NACK: invalid config -> REJECTED loudly; cfg=v2 keeps serving ==="
  echo "    (demo/gateway.invalid.yaml: unknown provider ref + placeholder typo)"
} >>"$OUT"
cp demo/gateway.invalid.yaml "$CFG"
kill -HUP "$GW_PID"
sleep 0.5
{
  echo "--- request AFTER the rejected reload: still served, still cfg=v2 ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/openai/v1/chat/completions" \
    -H 'x-attr-team: ml-research' \
    -H 'content-type: application/json' -d '{}'
} >>"$OUT"
sleep 2.5 # the poll watcher sees the same bad file: one more NACK, same path
{
  echo "--- gateway log: precise errors + still-active version, from BOTH triggers ---"
  gw_since "$M"
  echo
} >>"$OUT"
stop_mock

# --- (9) no-op: identical content is hash-detected ------------------------
M=$(mark)
cp demo/gateway.v2.yaml "$CFG" # restore the bytes cfg=v2 was rendered from
kill -HUP "$GW_PID"
sleep 0.5
{
  echo "=== (9) no-op reload: content identical to the active snapshot ==="
  echo "--- gateway log: hash check short-circuits at debug level, no new version ---"
  gw_since "$M"
  echo
} >>"$OUT"

# --- (10) GB-8: operator billing labels merged into the Vertex body -------
start_mock demo/vertex.sse vertex
M=$(mark)
{
  echo "=== (10) GB-8: operator labels merged into the generateContent BODY (cfg=v2) ==="
  echo "    /vertex composes fleet -> project ml-platform -> route -> app ml-research:"
  echo "    cost_center=platform-eng (static), team (from attribution),"
  echo "    env (from the FLEET pin: canary under cfg=v2), channel (CEL-derived)."
  echo "    The client tries to spoof cost_center and sends its own extra label;"
  echo "    x-echo-label-* shows what the UPSTREAM actually received in the body."
  echo "--- curl -s -D - (client body labels: cost_center spoofed, note kept) ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/vertex/v1/models/gemini-2.5-flash:streamGenerateContent" \
    -H 'x-attr-team: ml-research' -H 'content-type: application/json' \
    -d '{"contents":[{"parts":[{"text":"hi"}]}],"labels":{"cost_center":"client-spoof","note":"client-label"}}'
  echo "--- gateway log: [gb8] resolved labels + body merge ---"
  gw_since "$M"
  echo
} >>"$OUT"
stop_mock

# --- (11) GB-7: STS session-tag credentials, SigV4-verified ---------------
STS_LOG=$(mktemp)
"$BIN/mock_sts" --port 6199 2>"$STS_LOG" &
STS_PID=$!
"$BIN/mock_upstream" --port 6191 --fixture "$FIXTURES/bedrock.jsonl" \
  --provider bedrock --delay-ms 40 --require-sigv4 2>/dev/null &
BEDROCK_PID=$!
sleep 0.3
M=$(mark)
{
  echo "=== (11) GB-7: AssumeRole session tags from resolved attribution (cfg=v2) ==="
  echo "    /bedrock-sts pins billing_team=ml-platform; env is the fleet pin (canary)."
  echo "    The mock Bedrock REQUIRES a valid SigV4 signature and decodes the"
  echo "    session tags from the SECURITY TOKEN — x-echo-session-tag-* proves the"
  echo "    attribution rode the CREDENTIALS, not a header. Tag sources are"
  echo "    operator/attribution-derived ONLY; a caller-raw tag is a config error."
  echo "--- request 1: exchange + sign (expect cache=miss, ASIAMOCK0001) ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/bedrock-sts/model/anthropic.claude/converse-stream" \
    -H 'x-attr-team: ml-research' -H 'content-type: application/json' -d '{}'
  echo "--- request 2: same tag-set (expect cache=hit, SAME access key) ---"
  curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/bedrock-sts/model/anthropic.claude/converse-stream" \
    -H 'x-attr-team: ml-research' -H 'content-type: application/json' -d '{}'
  echo "--- mock-sts log: ONE AssumeRole for the two requests ---"
  cat "$STS_LOG"
  echo "--- gateway log: [gb7] exchange, cache hit, and the meter join ---"
  gw_since "$M"
} >>"$OUT"
kill "$STS_PID" "$BEDROCK_PID" 2>/dev/null || true
wait "$STS_PID" "$BEDROCK_PID" 2>/dev/null || true
rm -f "$STS_LOG"

echo "wrote $OUT"
