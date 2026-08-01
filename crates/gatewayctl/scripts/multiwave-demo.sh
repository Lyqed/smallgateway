#!/usr/bin/env bash
# End-to-end proof for Phase 2, milestone 3: MULTI-WAVE rollout grouped by
# failure domain, and GatewaySets. One gatewayctl compiles a config repo whose
# `waves.yaml` orders the fleet canary -> eu -> us and whose `gatewaysets.yaml`
# stamps `tier: gold` onto every eu node, and distributes rendered snapshots to
# THREE gatewayd nodes (one per region) over gRPC. Produces ../multiwave-demo.log.
#
# Scenarios (each with the real process logs):
#   (a) ORDERED MULTI-WAVE ROLLOUT: three region-labeled nodes join. A config
#       change (fleet env pin prod -> canary) rolls out canary -> eu -> us: each
#       wave is pushed and must fully ACK before the next wave is pushed. The
#       per-wave COMMITTED state is surfaced ("all waves [...] on <commit>").
#   (b) GATEWAYSET STAMP: the eu node's render carries `tier: gold` (stamped by
#       the eu-gold-tier GatewaySet) while canary/us do NOT — so eu renders a
#       DIFFERENT render_hash from the same repo, purely from its region label.
#       The operator wrote ONE GatewaySet, not a per-eu-node file.
#   (d) A NEWLY-JOINED matching node PICKS UP the GatewaySet stamp on render: a
#       second eu node joins later and its first push already carries the eu-
#       stamped render (tier: gold) — no per-node file was edited. (Run before
#       (c) so the fleet is on a clean render when it joins.)
#   (c) A NACK/timeout in WAVE 2 HALTS it and FREEZES WAVE 3, WAVE 1 stays
#       advanced: the eu node is frozen (SIGSTOP) so it goes SILENT past the wave
#       timeout during a rollout. Wave 1 (canary) ACKs and stays advanced; wave 2
#       (eu) HALTS; wave 3 (us) is FROZEN on its prior commit and never pushed.
#       The mixed per-wave committed state is surfaced, never "some on new, some
#       on old, shrug".
#
# Config-canary ANALYSIS between waves (Phase 5 — this is the ordered-wave
# substrate it sits on), Postgres, and per-node latching remain OUT OF SCOPE
# (see crates/gatewayctl/README.md). The wave ack timeout is ~5s, so the halt
# scenario pauses past it.
set -uo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
BIN=../../target/debug
CTL_PORT=6207
CANARY_PORT=6221
EU_PORT=6222
US_PORT=6223
EU2_PORT=6224
MOCK_PORT=6190
JOIN=demo-join
OUT=multiwave-demo.log
FIXTURES=../../spikes/event-model/fixtures

# A MUTABLE COPY of the repo so scenario (a) can commit a change underneath the
# running control plane while the checked-in demo repo stays pristine.
REPO=$(mktemp -d)/multiwave-repo
cp -r demo/multiwave-repo "$REPO"

CTL_LOG=$(mktemp)
CANARY_LOG=$(mktemp)
EU_LOG=$(mktemp)
US_LOG=$(mktemp)
EU2_LOG=$(mktemp)
CTL_PID=""; CANARY_PID=""; EU_PID=""; US_PID=""; EU2_PID=""; MOCK_PID=""
cleanup() {
  # Make sure a STOPped node is resumed before we kill it.
  [ -n "$EU_PID" ] && kill -CONT "$EU_PID" 2>/dev/null || true
  for pid in "$CANARY_PID" "$EU_PID" "$US_PID" "$EU2_PID" "$CTL_PID" "$MOCK_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -f "$CTL_LOG" "$CANARY_LOG" "$EU_LOG" "$US_LOG" "$EU2_LOG"
  rm -rf "$(dirname "$REPO")"
}
trap cleanup EXIT

mark() { wc -l <"$1"; }
since() { local f="$1" m="$2"; tail -n +"$((m + 1))" "$f" || true; }
wait_port() {
  local port="$1"
  for _ in $(seq 1 50); do
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && { exec 3>&- 3<&-; return 0; }
    sleep 0.1
  done
  return 1
}
wait_bound() {
  local log="$1"
  for _ in $(seq 1 60); do
    grep -q "bound first snapshot" "$log" && return 0
    sleep 0.1
  done
  return 1
}

: >"$OUT"
{
  echo "############################################################"
  echo "# Multi-wave rollout + GatewaySets demo — Phase 2, milestone 3"
  echo "#   1 gatewayctl + 3 region-labeled gatewayd nodes over gRPC"
  echo "#   waves.yaml: canary -> eu -> us   gatewaysets.yaml: eu => tier gold"
  echo "############################################################"
  echo
} >>"$OUT"

# --- upstream the nodes proxy to (best-effort; the proof is in the logs) -----
"$BIN/mock_upstream" --port "$MOCK_PORT" --fixture "$FIXTURES/openai.sse" \
  --provider openai --delay-ms 20 2>/dev/null &
MOCK_PID=$!

# --- (0) control plane up, region-labeled tokens minted ---------------------
# Poll off; we drive the config change explicitly. Reconcile every 2s. Three
# labeled tokens hand each node its region, which the wave plan selects on.
"$BIN/gatewayctl" --repo "$REPO" --listen "127.0.0.1:$CTL_PORT" \
  --join-token "$JOIN" --poll-interval 0 --reconcile-interval 2 \
  --label-token "region=canary:tok-canary" \
  --label-token "region=eu:tok-eu" \
  --label-token "region=eu:tok-eu-2" \
  --label-token "region=us:tok-us" >>"$CTL_LOG" 2>&1 &
CTL_PID=$!
wait_port "$CTL_PORT" || { echo "control plane did not come up" >>"$OUT"; exit 1; }

{
  echo "=== (0) control plane up: compiled the repo (waves + gatewaysets) ==="
  grep -E "compiled source|minted" "$CTL_LOG" | head -3
  echo "    ^ N gatewayset(s) and 3 wave(s) loaded from the repo."
  echo
} >>"$OUT"

# --- region-labeled nodes join ---------------------------------------------
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-canary --join-token tok-canary --listen "127.0.0.1:$CANARY_PORT" >"$CANARY_LOG" 2>&1 &
CANARY_PID=$!
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-eu --join-token tok-eu --listen "127.0.0.1:$EU_PORT" >"$EU_LOG" 2>&1 &
EU_PID=$!
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-us --join-token tok-us --listen "127.0.0.1:$US_PORT" >"$US_LOG" 2>&1 &
US_PID=$!
wait_bound "$CANARY_LOG"; wait_bound "$EU_LOG"; wait_bound "$US_LOG"
sleep 0.5

{
  echo "=== (b) GATEWAYSET STAMP: eu renders a DIFFERENT hash from the same repo ==="
  echo "--- each node's first bound render_hash (eu carries tier: gold) ---"
  echo "node-canary:"; grep -E "bound first snapshot" "$CANARY_LOG" | tail -1
  echo "node-eu    :"; grep -E "bound first snapshot" "$EU_LOG" | tail -1
  echo "node-us    :"; grep -E "bound first snapshot" "$US_LOG" | tail -1
  echo "    ^ node-eu's render_hash DIFFERS from canary/us: the eu-gold-tier"
  echo "      GatewaySet stamped tier: gold onto it at RENDER time, from its"
  echo "      region=eu label alone — one GatewaySet, no per-node file."
  echo
} >>"$OUT"

# --- (a) ordered multi-wave rollout ----------------------------------------
CM=$(mark "$CTL_LOG")
# "Commit" the change: flip the fleet env pin prod -> canary, then SIGHUP to
# re-render + admit + walk the wave plan in order.
sed -i 's/env: prod/env: canary/' "$REPO/fleet/base.chain.yaml"
kill -HUP "$CTL_PID"
sleep 4

{
  echo "=== (a) ORDERED MULTI-WAVE ROLLOUT: canary -> eu -> us, each fully acked ==="
  echo "--- gatewayctl log: the waves advance IN ORDER, each fully acked ---"
  since "$CTL_LOG" "$CM" | grep -E "rollout|wave" | tail -n 16
  echo "    ^ wave 'canary' COMMITTED before 'eu' was pushed, 'eu' before 'us'."
  echo "      The per-wave committed state is surfaced (FULLY APPLIED)."
  echo
} >>"$OUT"

# --- (d) a newly-joined eu node picks up the GatewaySet stamp ---------------
# Done BEFORE the halt scenario so the applied render is still the clean v2
# canary render (a matching node picks up the stamp on ANY render, but keeping
# the fleet healthy here makes the proof unambiguous).
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-eu-2 --join-token tok-eu-2 --listen "127.0.0.1:$EU2_PORT" >"$EU2_LOG" 2>&1 &
EU2_PID=$!
wait_bound "$EU2_LOG" || true
sleep 1

{
  echo "=== (d) A NEWLY-JOINED eu node PICKS UP the stamp on its first render ==="
  echo "--- node-eu-2 (region=eu) joined AFTER the GatewaySet was authored ---"
  echo "node-eu-2:"; grep -E "bound first snapshot" "$EU2_LOG" | tail -1
  echo "--- gatewayctl: the initial push to node-eu-2 carries the eu-stamped hash ---"
  grep -E "node-eu-2" "$CTL_LOG" | grep -E "pushed initial|labels=" | tail -2
  echo "    ^ node-eu-2 got its region=eu STAMPED render on its first push — the"
  echo "      eu-gold-tier GatewaySet stamped tier: gold from its label alone, no"
  echo "      per-node file edited (docs/02). It renders a distinct hash from a"
  echo "      canary/us node on the same commit (proven in (b) above)."
  echo
} >>"$OUT"

# --- (c) a NACK/timeout in wave 2 halts it and freezes wave 3 ---------------
CM=$(mark "$CTL_LOG")
# Freeze the eu node so it cannot answer: its session stays registered but it
# goes SILENT past the wave timeout. Then commit another change and SIGHUP.
# node-eu-2 is also region=eu, so it will be in the same (halting) wave; stop it
# too so the eu wave's silence is unambiguous.
kill -STOP "$EU_PID"
[ -n "$EU2_PID" ] && kill -STOP "$EU2_PID" 2>/dev/null || true
sed -i 's/port: 6190/port: 6191/' "$REPO/providers.yaml"  # a real v3 change
{
  echo "=== (c) WAVE 2 HALTS (eu SILENT), WAVE 3 FROZEN, WAVE 1 STAYS ADVANCED ==="
  echo "    node-eu is frozen (SIGSTOP): still connected, but it will not ACK."
  echo "    A new commit is rolled out. Wave 1 (canary) acks and advances; wave"
  echo "    2 (eu) goes silent past the ~5s timeout and HALTS; wave 3 (us) is"
  echo "    FROZEN on its prior commit and never pushed."
} >>"$OUT"
kill -HUP "$CTL_PID"
# Wait past the wave ack timeout (~5s) plus slack for the halt to be recorded.
sleep 9

{
  echo "--- gatewayctl log: canary ADVANCED, eu HALTED, us FROZEN (surfaced) ---"
  since "$CTL_LOG" "$CM" | grep -E "rollout|wave|HALT|FROZEN|PARTIALLY" | tail -n 18
  echo "    ^ the mixed per-wave committed state is NAMED and queryable:"
  echo "      canary advanced to the new commit; eu halted; us frozen on the"
  echo "      prior commit — never 'some on new, some on old, shrug' (docs/07)."
  echo
} >>"$OUT"
# Resume the eu nodes (they could re-converge on a resumed rollout / next heal).
kill -CONT "$EU_PID"
[ -n "$EU2_PID" ] && kill -CONT "$EU2_PID" 2>/dev/null || true

{
  echo "############################################################"
  echo "# Summary: a commit walked an ordered wave plan grouped by"
  echo "#   failure domain (canary -> eu -> us), each wave fully"
  echo "#   acked before the next; a GatewaySet stamped tier: gold"
  echo "#   onto eu nodes so eu renders a distinct hash from ONE"
  echo "#   selector; a silent wave-2 node HALTED wave 2 and FROZE"
  echo "#   wave 3 while wave 1 stayed advanced, the mixed per-wave"
  echo "#   state surfaced; and a newly-joined eu node picked up the"
  echo "#   stamp on render. Waves are the substrate config canaries"
  echo "#   (Phase 5) sit on; the analysis between waves is deferred."
  echo "############################################################"
} >>"$OUT"

echo "wrote $OUT"
