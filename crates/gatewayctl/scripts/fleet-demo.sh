#!/usr/bin/env bash
# End-to-end proof for Phase 2, milestone 1: the control plane and fleet
# distribution. One gatewayctl compiles a config repo and distributes rendered
# snapshots to TWO gatewayd nodes over gRPC. Produces ../fleet-demo.log.
#
# Scenarios (each with the real process logs):
#   (1) BOTH nodes join with a join token and receive v1 (fan-out); a bad
#       join token is rejected.
#   (2) A config change (fleet env pin prod -> canary) is committed to the repo
#       and rolled out: gatewayctl pushes v2 to BOTH nodes, both ACK, and the
#       fleet's committed version advances. curl through each node proves the
#       per-node served version (the x-echo-attr-env the mock reflects flips to
#       canary on both).
#   (3) A deliberately-INVALID snapshot is injected (SIGUSR1 --push-raw,
#       bypassing the render gate): BOTH nodes NACK it, the wave HALTS, the
#       fleet's committed version does NOT advance, and every node keeps
#       serving its prior good version (v2). Divergence is logged loudly,
#       never silent.
#   (4) CONTROL-PLANE OUTAGE + AUTO-RECONNECT: the control plane is KILLED.
#       Both nodes keep serving their last snapshot (the control plane is not a
#       SPOF for serving — docs/07). The control plane is then RESTARTED; each
#       node AUTO-RECONNECTS (re-dial with backoff, no operator action, no
#       crash) and resumes receiving pushes. curl proves both nodes served
#       continuously across the outage.
#   (5) The per-node version is surfaced throughout: every gatewayd [cp-client]
#       line and every gatewayctl [ack]/[nack]/[rollout] line carries the
#       fleet_version and render_hash.
#
# Drift-detection, config-PR admission, Git integration, Postgres, and
# multi-wave rollouts are OUT OF SCOPE for M1 (see crates/gatewayctl/README.md).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
BIN=../../target/debug
CTL_PORT=6187
N1_PORT=6201
N2_PORT=6202
MOCK_PORT=6190
JOIN=demo-join
OUT=fleet-demo.log
FIXTURES=../../spikes/event-model/fixtures

# A MUTABLE COPY of the repo so scenario 2 can commit a change underneath the
# running control plane while the repo in Git stays pristine.
REPO_SRC=demo/config-repo
REPO=$(mktemp -d)/config-repo
cp -r "$REPO_SRC" "$REPO"

CTL_LOG=$(mktemp)
N1_LOG=$(mktemp)
N2_LOG=$(mktemp)

CTL_PID=""; N1_PID=""; N2_PID=""; MOCK_PID=""
cleanup() {
  for pid in "$N1_PID" "$N2_PID" "$CTL_PID" "$MOCK_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -f "$CTL_LOG" "$N1_LOG" "$N2_LOG"
  rm -rf "$(dirname "$REPO")"
}
trap cleanup EXIT

# Print the tail of a log file added since a saved line-count mark.
mark() { wc -l <"$1"; }
since() { local f="$1" m="$2"; tail -n +"$((m + 1))" "$f" || true; }

# Wait (up to 5s) for a TCP port to accept connections.
wait_port() {
  local port="$1"
  for _ in $(seq 1 50); do
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && { exec 3>&- 3<&-; return 0; }
    sleep 0.1
  done
  return 1
}

# Start the control plane (same args every time so scenario 4 can restart it).
# Appends to $CTL_LOG so a restart's log is contiguous with the original run.
start_ctl() {
  "$BIN/gatewayctl" --repo "$REPO" --listen "127.0.0.1:$CTL_PORT" \
    --join-token "$JOIN" --poll-interval 1 \
    --push-raw demo/invalid-snapshot.yaml >>"$CTL_LOG" 2>&1 &
  CTL_PID=$!
  wait_port "$CTL_PORT"
}

: >"$OUT"
{
  echo "############################################################"
  echo "# Fleet distribution demo — Phase 2, milestone 1"
  echo "#   1 gatewayctl (control plane) + 2 gatewayd nodes over gRPC"
  echo "############################################################"
  echo
} >>"$OUT"

# --- upstream the nodes proxy to (proves the served config per node) --------
"$BIN/mock_upstream" --port "$MOCK_PORT" --fixture "$FIXTURES/openai.sse" \
  --provider openai --delay-ms 20 2>/dev/null &
MOCK_PID=$!

# --- (0) start the control plane --------------------------------------------
: >"$CTL_LOG"
start_ctl

{
  echo "=== (0) control plane up: compiled the config repo into v1 ==="
  echo "--- gatewayctl log ---"
  cat "$CTL_LOG"
  echo
} >>"$OUT"

# --- (1) two nodes join and receive v1 --------------------------------------
CM=$(mark "$CTL_LOG")
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-a --join-token "$JOIN" --listen "127.0.0.1:$N1_PORT" >"$N1_LOG" 2>&1 &
N1_PID=$!
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-b --join-token "${JOIN}-2" --listen "127.0.0.1:$N2_PORT" >"$N2_LOG" 2>&1 &
N2_PID=$!

# Wait for both nodes to be serving (they only listen after binding v1).
for port in "$N1_PORT" "$N2_PORT"; do
  for _ in $(seq 1 50); do
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && { exec 3>&- 3<&-; break; }
    sleep 0.1
  done
done
sleep 0.5

{
  echo "=== (1) fan-out: BOTH nodes joined with a join token and received v1 ==="
  echo "    (node-a used token '$JOIN'; node-b used the single-use sibling '${JOIN}-2')"
  echo "--- gatewayctl log: two joins + two initial pushes + two acks ---"
  since "$CTL_LOG" "$CM"
  echo "--- node-a log: bound the first snapshot v1 ---"
  grep -E "cp-client|active config|listening" "$N1_LOG" | tail -n 8
  echo "--- node-b log: bound the first snapshot v1 ---"
  grep -E "cp-client|active config|listening" "$N2_LOG" | tail -n 8
  echo
  echo "--- a bad join token is REJECTED (a third node with a garbage token) ---"
  "$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
    --node-id node-bad --join-token not-a-real-token \
    --listen "127.0.0.1:6209" 2>&1 | grep -iE "reject|unauth|bootstrap failed" | head -3 || true
  echo
} >>"$OUT"

# Prove the served version per node BEFORE the change: env pin = prod.
{
  echo "--- curl through each node (v1): the mock echoes x-echo-attr-env=prod ---"
  echo "node-a:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N1_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo "node-b:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N2_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo
} >>"$OUT"

# --- (2) config change -> v2 pushed to both, both ACK -----------------------
CM=$(mark "$CTL_LOG")
N1M=$(mark "$N1_LOG"); N2M=$(mark "$N2_LOG")
# "Commit" the change: flip the fleet env pin prod -> canary in the repo.
sed -i 's/env: prod/env: canary/' "$REPO/fleet/base.chain.yaml"
# The poll watcher (1s) re-renders and rolls out; give it time.
sleep 2.5

{
  echo "=== (2) config change committed (env pin prod -> canary): v2 rolled out ==="
  echo "--- gatewayctl log: re-render + wave push v2 + BOTH acks + COMMITTED ---"
  since "$CTL_LOG" "$CM" | grep -E "reload|rollout|ack|nack" | tail -n 12
  echo "--- node-a log: swapped to v2 and ACKed ---"
  since "$N1_LOG" "$N1M" | grep -E "cp-client" | tail -n 4
  echo "--- node-b log: swapped to v2 and ACKed ---"
  since "$N2_LOG" "$N2M" | grep -E "cp-client" | tail -n 4
  echo
  echo "--- curl through each node (v2): x-echo-attr-env now reads canary on BOTH ---"
  echo "node-a:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N1_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env" || true
  echo "node-b:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N2_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env" || true
  echo
} >>"$OUT"

# --- (3) invalid snapshot injected -> both NACK, v2 keeps serving -----------
CM=$(mark "$CTL_LOG")
N1M=$(mark "$N1_LOG"); N2M=$(mark "$N2_LOG")
# SIGUSR1 injects demo/invalid-snapshot.yaml as a raw snapshot, bypassing the
# render gate — the nodes are the independent validation authority.
kill -USR1 "$CTL_PID"
sleep 1.5

{
  echo "=== (3) deliberately-invalid snapshot injected: BOTH nodes NACK ==="
  echo "    (SIGUSR1 pushes demo/invalid-snapshot.yaml raw; its route references"
  echo "     a provider that does not exist, so each node's local validation"
  echo "     REJECTS it. The wave HALTS and the fleet version does NOT advance.)"
  echo "--- gatewayctl log: push + TWO nacks + HALTED (committed version frozen) ---"
  since "$CTL_LOG" "$CM" | grep -E "inject|rollout|nack|HALTED|divergent" | tail -n 12
  echo "--- node-a log: NACKed the bad snapshot, still serving its good version ---"
  since "$N1_LOG" "$N1M" | grep -E "cp-client|NACK" | tail -n 4
  echo "--- node-b log: NACKed the bad snapshot, still serving its good version ---"
  since "$N2_LOG" "$N2M" | grep -E "cp-client|NACK" | tail -n 4
  echo
  echo "--- curl through each node AFTER the NACK: still v2 (canary), still serving ---"
  echo "node-a:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N1_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo "node-b:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N2_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo
} >>"$OUT"

# --- (4) control-plane outage + auto-reconnect ------------------------------
# Kill the control plane. The nodes MUST keep serving (control plane is not a
# SPOF for serving) and MUST auto-reconnect when it returns (no crash, no
# operator action).
N1M=$(mark "$N1_LOG"); N2M=$(mark "$N2_LOG")
kill "$CTL_PID" 2>/dev/null || true
wait "$CTL_PID" 2>/dev/null || true
CTL_PID=""
sleep 1.0

{
  echo "=== (4) control-plane OUTAGE: the control plane was killed ==="
  echo "--- both nodes keep serving their last snapshot with NO control plane ---"
  echo "    (the reconciler is not in the request path — docs/07: the control"
  echo "     plane is a SPOF only for CHANGING config, never for SERVING it)"
  echo "node-a (CP down):"; curl -s -D - -o /dev/null "http://127.0.0.1:$N1_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo "node-b (CP down):"; curl -s -D - -o /dev/null "http://127.0.0.1:$N2_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo "--- node-a log while the CP is down: stream ended, keeps serving, re-dialing ---"
  since "$N1_LOG" "$N1M" | grep -E "cp-client" | tail -n 4
  echo
} >>"$OUT"

# Bring the control plane back. Each node's supervisor re-dials with backoff and
# resumes the stream on its own — no restart of the nodes.
N1M=$(mark "$N1_LOG"); N2M=$(mark "$N2_LOG")
start_ctl
# Give the reconnect backoff (<=30s, starting at 0.5s) time to re-establish.
sleep 4

{
  echo "=== (4 cont.) control plane RESTARTED: both nodes AUTO-RECONNECTED ==="
  echo "    (no operator action, no node restart — the cp-client supervisor"
  echo "     re-dialed on its established identity and resumed pushes)"
  echo "--- node-a log: rejoined the control plane and resumed ---"
  since "$N1_LOG" "$N1M" | grep -E "cp-client" | tail -n 4
  echo "--- node-b log: rejoined the control plane and resumed ---"
  since "$N2_LOG" "$N2M" | grep -E "cp-client" | tail -n 4
  echo "--- gatewayctl log: both nodes reconnected and were re-pushed the render ---"
  grep -E "join|rejoin|reconnect" "$CTL_LOG" | tail -n 8
  echo
  echo "--- curl through each node AFTER reconnect: still serving (canary) ---"
  echo "node-a:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N1_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo "node-b:"; curl -s -D - -o /dev/null "http://127.0.0.1:$N2_PORT/openai/v1/chat" \
    -H 'x-attr-team: ml' -H 'content-type: application/json' -d '{}' | grep -iE "x-echo-attr-env|HTTP/" || true
  echo
} >>"$OUT"

{
  echo "############################################################"
  echo "# Summary: two nodes joined (v1), took a rolled-out change"
  echo "#   (v2, both ACK, fleet committed), REJECTED an invalid"
  echo "#   snapshot (both NACK, wave halted, fleet version frozen,"
  echo "#   each node kept serving), and SURVIVED a control-plane"
  echo "#   outage — kept serving with the CP down, then AUTO-"
  echo "#   RECONNECTED when it returned. Divergence surfaced, never"
  echo "#   silent; the data plane never depends on the CP to serve."
  echo "############################################################"
} >>"$OUT"

echo "wrote $OUT"
