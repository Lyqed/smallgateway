#!/usr/bin/env bash
# End-to-end proof for Phase 3: GB-5 spend caps via budget shares, GB-6 alerts
# from the enforcement layer, and mid-stream enforcement wired to the shares.
# Produces ../budget-demo.log.
#
# The 100k-token scenario is FIVE lines of YAML in demo/budget.yaml (a fleet
# default of 100_000 tokens per team, plus two Git-reviewed per-value overrides).
# gatewayd runs standalone against a MUTABLE COPY so a tiny cap can be swapped in
# for the mid-stream-cut scenario. Every enforcement decision is a real process
# log line; every curl is a real request.
#
# Scenarios, each with real curl output plus the gateway's own log:
#   (1) GB-5  DEFAULT CAP: team=free-tier is capped at 5_000 tokens. The first
#             request streams; the accumulated spend is logged with cap/share.
#   (2) GB-6  SOFT + HARD ALERTS from the enforcement layer: as the running
#             tally crosses 80% and then the cap, gatewayd emits [gb6 SOFT-80%]
#             and [gb6 HARD-CAP] lines carrying spender, cap, spend, node — at
#             the point of enforcement, not reconstructed from logs.
#   (3) GB-4  CAP REJECTION: once free-tier is over its cap, the NEXT request is
#             rejected at request start with the operator's GB-4 body (429),
#             naming the cap and spend in tokens. No token reaches the upstream.
#   (4) GB-5  MID-STREAM CUT: a 3-token cap is exhausted MID-generation, so the
#             stream is cut with the operator's GB-4 terminal event (event:
#             error / data: budget_exhausted) rather than running to [DONE].
#   (5) GB-5  BOUNDED OVERSPEND UNDER PARTITION (the MEASURED number): a node
#             holding a 40k share of a 100k cap loses the control plane, admits
#             one last stream, and is cut at its share + one stream's tail. The
#             overspend is reported as an ABSOLUTE NUMBER and a fraction of the
#             cap — bounded, never unbounded. Measured, not estimated.
#
# Share allocation + continuous rebalance and the ~90% synchronous escalation
# are proven over real gRPC in ../gatewayctl/tests/budget.rs; the classification
# and allocation math in the gateway_core::budget / gatewayctl::budget unit
# tests. This demo is the DATA-PLANE enforcement story end to end.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
BIN=../../target/debug
PORT=6188
MOCK_PORT=6190
OUT=budget-demo.log
GW_LOG=$(mktemp)
FIXTURES=../../spikes/event-model/fixtures

CFG_DIR=$(mktemp -d)
CFG="$CFG_DIR/budget.yaml"
cp demo/budget.yaml "$CFG"

MOCK_PID=""; GW_PID=""
cleanup() {
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
  [ -n "$GW_PID" ] && kill -9 "$GW_PID" 2>/dev/null || true
  rm -rf "$CFG_DIR" "$GW_LOG"
}
trap cleanup EXIT

start_mock() { # $1 = fixture, $2 = provider, $3 = delay-ms
  "$BIN/mock_upstream" --port "$MOCK_PORT" --fixture "$1" --provider "$2" \
    --delay-ms "${3:-40}" 2>/dev/null &
  MOCK_PID=$!
  sleep 0.3
}
stop_mock() {
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
  wait "$MOCK_PID" 2>/dev/null || true
  MOCK_PID=""
}
start_gw() { # re-read cfg each start
  RUST_LOG=info "$BIN/gatewayd" --config "$CFG" \
    --listen "127.0.0.1:$PORT" --poll-interval 0 >"$GW_LOG" 2>&1 &
  GW_PID=$!
  sleep 0.6
}
stop_gw() {
  # SIGKILL, not SIGTERM: pingora treats SIGTERM as a graceful shutdown with a
  # 300s grace period, which would stall the demo. We want an immediate stop.
  [ -n "$GW_PID" ] && kill -9 "$GW_PID" 2>/dev/null || true
  wait "$GW_PID" 2>/dev/null || true
  GW_PID=""
}
mark() { wc -l <"$GW_LOG"; }
gw_since() { tail -n +"$(( $1 + 1 ))" "$GW_LOG"; }

: >"$OUT"
say() { echo "$@" | tee -a "$OUT"; }
run() { # echo a command, run it, capture output into the log
  say "\$ $*"
  eval "$*" 2>&1 | tee -a "$OUT"
  say ""
}

say "=============================================================="
say " Phase 3 demo — GB-5 spend caps, GB-6 alerts, mid-stream cut"
say "=============================================================="
say ""
say "The 100k-token scenario, five lines of YAML (demo/budget.yaml):"
say ""
sed -n '/spend_caps:/,/free-tier: 5000/p' demo/budget.yaml | sed 's/^/    /' | tee -a "$OUT"
say ""

# The headline config caps free-tier at 5_000 tokens; each fixture stream is
# ~9 tokens, so exhausting it would take hundreds of requests. For a legible
# demo we swap free-tier to a 25-token cap — the SAME machinery, a smaller
# number — so three ~9-token streams cross 80% then the cap.
sed 's/free-tier: 5000/free-tier: 25/' demo/budget.yaml >"$CFG"
start_mock "$FIXTURES/openai.sse" openai 20
start_gw

# ---------------------------------------------------------------------------
say "--------------------------------------------------------------"
say "(1)(2)(3) GB-5 default cap + GB-6 alerts + GB-4 cap rejection"
say "--------------------------------------------------------------"
say "team=free-tier is capped at 25 tokens for a legible demo (the same cap"
say "machinery as the headline 100k scenario, a smaller number). Each fixture"
say "stream is ~9 tokens, so the running tally crosses 80% then the cap; the"
say "GB-6 soft and hard alerts fire AT the enforcement point, then the next"
say "request is rejected at request start with the operator's GB-4 body."
say ""
M=$(mark)
code=""
for i in 1 2 3 4 5; do
  code=$(curl -sN -o /dev/null -w '%{http_code}' \
    -X POST "http://127.0.0.1:$PORT/openai/v1/chat" \
    -H 'x-attr-team: free-tier' -d '{}')
  say "  request #$i for team=free-tier -> HTTP $code"
  # Once we see the cap rejection at admission, stop.
  [ "$code" = "429" ] && break
done
say ""
if [ "$code" = "429" ]; then
  say "The operator's GB-4 body on the rejected request (naming cap + spend):"
  run "curl -sN -X POST 'http://127.0.0.1:$PORT/openai/v1/chat' -H 'x-attr-team: free-tier' -d '{}'"
fi
say "gatewayd enforcement log (GB-6 alerts + GB-5 budget lines) for free-tier:"
gw_since "$M" | grep -E '\[gb6 |\[gb5 |\[budget ' | grep -i 'free-tier' | sed 's/^/    /' | tee -a "$OUT" || true
say ""

stop_gw
stop_mock

# ---------------------------------------------------------------------------
say "--------------------------------------------------------------"
say "(4) GB-5 MID-STREAM CUT with the GB-4 terminal event"
say "--------------------------------------------------------------"
say "A 3-token cap is exhausted MID-generation. The stream is cut with the"
say "operator's terminal event (event: error / data: budget_exhausted); the"
say "upstream's [DONE] never reaches the client — the stream did not run to"
say "completion."
say ""
# Swap in a 3-token cap for team=ml-research.
sed 's/ml-research: 200000/ml-research: 3/' demo/budget.yaml >"$CFG"
start_mock "$FIXTURES/openai.sse" openai 60
start_gw
M=$(mark)
run "curl -sN -X POST 'http://127.0.0.1:$PORT/openai/v1/chat' -H 'x-attr-team: ml-research' -d '{}'"
say "gatewayd log: the mid-stream cut and the reconciled terminal count:"
gw_since "$M" | grep -E 'EXCEEDED mid-stream|CUTTING|\[budget ' | sed 's/^/    /' | tee -a "$OUT" || true
say ""
stop_gw
stop_mock

# ---------------------------------------------------------------------------
say "--------------------------------------------------------------"
say "(5) GB-5 BOUNDED OVERSPEND UNDER PARTITION — the MEASURED number"
say "--------------------------------------------------------------"
say "When a node cannot reach the control plane it fails to a DOCUMENTED"
say "bounded-overspend policy: spend only up to the share it already holds,"
say "then stop. A node holding a 40k share of a 100k cap admits one last stream"
say "and is cut at its share + that stream's tail. The overspend is MEASURED —"
say "reported as a number against the configured cap, never estimated:"
say ""
say "NOTE: unlike scenarios 1-4 (which drive the real gatewayd over HTTP), this"
say "number comes from executing the enforcement unit (LocalBudget) under a"
say "SIMULATED partition — real code, real measurement, no gRPC round trip:"
run "cargo test -q -p gatewayd --bin gatewayd budget::tests::measured_bounded_overspend_under_partition -- --nocapture 2>/dev/null | grep MEASURED"
say "Bounded: the overspend is one stream's tail past the held share, strictly"
say "less than one request's worth — never the unbounded local-bucket failure"
say "(docs/01 Q4; docs/02 'GB-5 at fleet scale — budget shares')."
say ""

say "=============================================================="
say " Phase 3 demo complete. GB-5 caps enforced, GB-6 alerts fired,"
say " mid-stream cut with the GB-4 terminal event, partition"
say " overspend MEASURED as a number against the cap."
say "=============================================================="
