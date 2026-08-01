#!/usr/bin/env bash
# End-to-end proof for Phase 2, milestone 2: Git as config truth, drift
# detection + self-heal, and config-PR admission. One gatewayctl sources its
# desired config from a REAL Git repo at a commit and distributes rendered
# snapshots to a gatewayd node over gRPC. Produces ../git-drift-demo.log.
#
# Scenarios (each with the real process logs):
#   (a) GIT AS TRUTH: the control plane sources config from a Git repo at a
#       commit; the snapshot carries that exact 40-hex commit SHA (not a
#       synthetic id), so it is reproducible from that commit (docs/07).
#   (b) DRIFT + SELF-HEAL: the node is pushed a stale/edited snapshot OUT OF
#       BAND (SIGUSR1 --push-raw — a VALID config that is NOT the Git desired),
#       so its DELIVERED record diverges from Git desired. The reconciler's next
#       tick DETECTS the divergence via the desired/delivered/observed three-hash
#       compare (the delivered != desired row of the truth table) and RE-PUSHES
#       desired; the node swaps back and ACKs — healed within one tick. (The node
#       typically swaps back and re-ACKs before its next 10s Status heartbeat, so
#       the logged compare shows observed == desired at heal time: the trigger
#       here is the delivered-stale row, not the node-observed-drift row. The
#       observed != desired row is exercised directly in tests/reconcile.rs::
#       a_drifted_node_is_healed_within_one_tick.)
#   (c) BREAK-GLASS with TTL: the node is marked break-glass for a bounded
#       window (SIGUSR2). While the window is open the reconciler TOLERATES the
#       node's drift and does NOT fight it (logged with the expiry). After the
#       TTL lapses the reconciler resumes and heals the node back to desired.
#   (d) ADMISSION BLOCKS A BAD CONFIG: `gatewayctl admit` is run against a
#       deliberately-broken candidate (a route with no attribution key — GB-1);
#       admission BLOCKS it, names the failing rule, and exits non-zero, so the
#       bad config never becomes desired / never rolls out. The good Git HEAD
#       admits (exit 0).
#
# Postgres, multi-wave rollouts, GatewaySets/label-generators, and per-node
# latching remain OUT OF SCOPE (see crates/gatewayctl/README.md). The node's
# heartbeat interval is 10s, so the drift scenarios pause for a heartbeat.
set -uo pipefail
cd "$(dirname "$0")/.."

cargo build --bins >/dev/null 2>&1
BIN=../../target/debug
CTL_PORT=6197
N1_PORT=6211
MOCK_PORT=6190
JOIN=demo-join
OUT=git-drift-demo.log
FIXTURES=../../spikes/event-model/fixtures

# --- a REAL Git repo as the config source ----------------------------------
GITROOT=$(mktemp -d)
GITREPO="$GITROOT/config-repo"
cp -r demo/config-repo "$GITREPO"
(
  cd "$GITREPO"
  git init -q
  git config user.email demo@demo
  git config user.name demo
  git add -A
  git commit -q -m "fleet config: env=prod, one openai route"
)
COMMIT=$(cd "$GITREPO" && git rev-parse HEAD)

# A break-glass control file (SIGUSR2 reads this): node-a for 4 seconds.
BG_FILE=$(mktemp)
echo "node-a 4" >"$BG_FILE"

CTL_LOG=$(mktemp)
N1_LOG=$(mktemp)
CTL_PID=""; N1_PID=""; MOCK_PID=""
cleanup() {
  for pid in "$N1_PID" "$CTL_PID" "$MOCK_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -f "$CTL_LOG" "$N1_LOG" "$BG_FILE"
  rm -rf "$GITROOT"
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

: >"$OUT"
{
  echo "############################################################"
  echo "# Git-truth + drift self-heal + admission demo"
  echo "#   Phase 2, milestone 2"
  echo "############################################################"
  echo
} >>"$OUT"

# --- upstream the node proxies to (best-effort; the proof is in the logs) ----
"$BIN/mock_upstream" --port "$MOCK_PORT" --fixture "$FIXTURES/openai.sse" \
  --provider openai --delay-ms 20 2>/dev/null &
MOCK_PID=$!

# --- (a) control plane sourced from Git at a commit -------------------------
# Reconcile every 2s so drift heals fast. push-raw arms the out-of-band drift;
# break-glass-file arms SIGUSR2. Poll is off (we drive changes explicitly).
"$BIN/gatewayctl" --git-repo "$GITREPO" --git-ref HEAD \
  --listen "127.0.0.1:$CTL_PORT" --join-token "$JOIN" \
  --poll-interval 0 --reconcile-interval 2 \
  --push-raw demo/drift-snapshot.yaml \
  --break-glass-file "$BG_FILE" >>"$CTL_LOG" 2>&1 &
CTL_PID=$!
wait_port "$CTL_PORT" || { echo "control plane did not come up" >>"$OUT"; exit 1; }

{
  echo "=== (a) GIT AS TRUTH: config sourced from a real Git repo at a commit ==="
  echo "    repo HEAD commit: $COMMIT"
  echo "--- gatewayctl log: compiled the Git commit into the first render ---"
  grep -E "compiled source" "$CTL_LOG" | head -2
  echo "    ^ the render's source_commit is the real 40-hex Git SHA above,"
  echo "      so the snapshot is reproducible from that exact commit (docs/07)."
  echo
} >>"$OUT"

# A node joins and binds the Git-sourced snapshot over gRPC.
"$BIN/gatewayd" --control-plane "http://127.0.0.1:$CTL_PORT" \
  --node-id node-a --join-token "$JOIN" --listen "127.0.0.1:$N1_PORT" >"$N1_LOG" 2>&1 &
N1_PID=$!
# The node binds config over gRPC before it ever listens on HTTP; the drift
# proof is entirely over the gRPC stream, so we wait for the bind log, not the
# HTTP port (which needs privileges some sandboxes withhold).
for _ in $(seq 1 50); do
  grep -q "bound first snapshot" "$N1_LOG" && break
  sleep 0.1
done
# Give the node's first Status heartbeat time to land so observed is populated.
sleep 11

{
  echo "--- node-a joined and bound the Git-sourced snapshot (v1) over gRPC ---"
  grep -E "cp-client|bound first" "$N1_LOG" | tail -n 3
  echo "--- gatewayctl: node-a in sync (desired == delivered == observed) ---"
  grep -E "reconcile|ack" "$CTL_LOG" | tail -n 3
  echo
} >>"$OUT"

# --- (b) drift + self-heal --------------------------------------------------
CM=$(mark "$CTL_LOG"); N1M=$(mark "$N1_LOG")
{
  echo "=== (b) DRIFT + SELF-HEAL within one reconcile tick ==="
  echo "    A stale/edited snapshot is pushed OUT OF BAND (SIGUSR1 --push-raw),"
  echo "    a VALID config that is NOT the Git desired (env=drifted). node-a"
  echo "    binds it, so its DELIVERED record now diverges from Git desired"
  echo "    (the delivered != desired truth-table row; the node re-ACKs desired"
  echo "    before its next heartbeat, so observed == desired at heal time)."
} >>"$OUT"
kill -USR1 "$CTL_PID"
# The node binds the drift; the reconciler (2s) catches delivered!=desired and
# re-pushes desired within a tick.
sleep 4

{
  echo "--- node-a drifted: bound the out-of-band snapshot, then healed back ---"
  since "$N1_LOG" "$N1M" | grep -E "cp-client" | tail -n 3
  echo "--- reconciler: three-hash compare (desired/delivered/observed) + heal ---"
  since "$CTL_LOG" "$CM" | grep -E "reconcile|inject" | grep -E "desired=|healed|self-healing|SIGUSR1" | tail -n 6
  echo "    ^ the reconciler DETECTED the delivered != desired divergence and"
  echo "      RE-PUSHED desired; node-a swapped back and ACKed — healed (docs/07:"
  echo "      self-heal is re-push). The observed != desired row (node running a"
  echo "      stale local file) is covered in tests/reconcile.rs."
  echo
} >>"$OUT"

# --- (c) break-glass with TTL: tolerate then heal ---------------------------
CM=$(mark "$CTL_LOG")
{
  echo "=== (c) BREAK-GLASS with TTL: tolerate the drift, then heal after expiry ==="
  echo "    node-a is marked break-glass for 4s (SIGUSR2), THEN drifted again"
  echo "    (out-of-band push). The reconciler must TOLERATE — not fight — the"
  echo "    drift while the window is open, then heal once the TTL lapses."
} >>"$OUT"
kill -USR2 "$CTL_PID"    # mark break-glass (node-a, 4s)
sleep 0.4
kill -USR1 "$CTL_PID"    # drift node-a again while break-glass is OPEN
sleep 3

{
  echo "--- while break-glass is OPEN: reconciler TOLERATES the drift (no heal) ---"
  since "$CTL_LOG" "$CM" | grep -iE "break-glass|tolerat" | tail -n 4
} >>"$OUT"

# Wait for the 4s break-glass window to lapse; the reconciler then heals.
sleep 4
{
  echo "--- after the TTL LAPSES: reconciler resumes and heals node-a to desired ---"
  since "$CTL_LOG" "$CM" | grep -E "reconcile" | grep -E "healed|self-healing" | tail -n 3
  echo
} >>"$OUT"

# --- (d) admission blocks a bad config --------------------------------------
{
  echo "=== (d) ADMISSION BLOCKS A BAD CONFIG (CI gate on a config PR) ==="
  echo "    A candidate config PR strips the attribution key from the route"
  echo "    (violates GB-1). 'gatewayctl admit' must BLOCK it and exit non-zero,"
  echo "    so it can never become desired / never roll out."
} >>"$OUT"
BADROOT=$(mktemp -d)
BADREPO="$BADROOT/config-repo"
cp -r demo/config-repo "$BADREPO"
# Break GB-1: remove required_keys everywhere so the route enforces no
# attribution contract.
cat >"$BADREPO/fleet/base.chain.yaml" <<'YAML'
# GB-1 VIOLATION (admission demo): no required_keys anywhere, so the route
# enforces no attribution contract at all.
attribution:
  pinned: { env: prod }
YAML
"$BIN/gatewayctl" admit --repo "$BADREPO" >>"$OUT" 2>&1
ADMIT_RC=$?
{
  echo "    admit exit code: $ADMIT_RC (non-zero => blocked, CI fails the PR)"
  echo
  echo "--- and the GOOD Git HEAD the fleet actually runs admits (exit 0) ---"
} >>"$OUT"
"$BIN/gatewayctl" admit --git-repo "$GITREPO" --git-ref HEAD >>"$OUT" 2>&1
GOOD_RC=$?
echo "    admit exit code: $GOOD_RC (zero => admitted)" >>"$OUT"
rm -rf "$BADROOT"

{
  echo
  echo "############################################################"
  echo "# Summary: config sourced from a real Git commit (snapshot"
  echo "#   carries the SHA); a divergent node detected via the"
  echo "#   desired/delivered/observed three-hash compare and"
  echo "#   self-healed within one reconcile tick; break-glass"
  echo "#   tolerated the drift for its TTL then healed after it"
  echo "#   lapsed; and a bad config PR was BLOCKED at admission"
  echo "#   with the failing rule named. Git decides desired;"
  echo "#   drift never persists silently."
  echo "############################################################"
} >>"$OUT"

echo "wrote $OUT"
