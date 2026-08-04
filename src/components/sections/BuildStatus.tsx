import { HandArrow } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

/** Honest, dated status — sourced from docs/04-build-plan.md (the Phase 0
 * closed callout), the spike READMEs, and the repo README as of
 * 2026-08-01. No invented progress — and no withheld progress either. */

type ChipTone = "teal" | "gold";

const CHIP_TONES: Record<ChipTone, { dot: string; text: string }> = {
  teal: { dot: "bg-teal", text: "text-teal-deep" },
  gold: { dot: "bg-gold", text: "text-gold-deep" },
};

function StatusChip({ tone, label }: { tone: ChipTone; label: string }) {
  const styles = CHIP_TONES[tone];
  return (
    <span
      className={`voice-mono inline-flex items-center gap-1.5 rounded-sm border border-steel bg-[oklch(99%_0.002_95)] px-2.5 py-1 text-[0.7rem] ${styles.text}`}
    >
      <span aria-hidden className={`size-1.5 rounded-full ${styles.dot}`} />
      {label}
    </span>
  );
}

const PHASES = [
  {
    id: "0",
    title: "The spikes",
    note: "Canonical event model over OpenAI, Anthropic, and Bedrock; the metering error bound measured from 17 real transcripts; Pingora chosen for the data plane.",
    state: "done" as const,
  },
  {
    id: "1",
    title: "Standalone data plane",
    note: "Pingora proxy enforcing GB-1 through GB-4, GB-7, and GB-8; scoped policy chain (fleet, project, route, app); CEL tier-1; hot-reload; a conformance suite that runs against a real gatewayd and writes machine-readable results.",
    state: "done" as const,
  },
  {
    id: "2",
    title: "Control plane",
    note: "Fleet distribution of rendered snapshots over gRPC with join-token auth and ACK/NACK; Git as the source of truth, reproducible from a commit; multi-wave rollout grouped by failure domain with halt-and-freeze; a domain-aware drift reconciler with self-heal; break-glass with TTL; config-PR admission; GatewaySets and tenancy scoping.",
    state: "done" as const,
  },
  {
    id: "3",
    title: "The stateful layer",
    note: "GB-5 fleet spend caps via budget shares, ~90% synchronous escalation so the common path has no per-request hop and no SPOF, bounded overspend under partition measured, shares provably summing to at most the cap; GB-6 alerts firing from the enforcement layer; mid-stream enforcement that cuts a stream with the GB-4 terminal event when a budget is exhausted.",
    state: "done" as const,
  },
  {
    id: "4",
    title: "WASM SDK and GB-9 hot swap",
    note: "wasmtime tier-2 modules, sandboxed with no ambient I/O, per-invocation fuel plus epoch preemption so a bad module fails closed; signed modules only; the per-event cost measured at ~11us and the honest call encoded (per-event streaming hooks gated off behind that budget); GB-9 atomic module-and-config binding, per-stream drain, versioned counter-schema migration.",
    state: "done" as const,
  },
  {
    id: "5",
    title: "Config canaries",
    note: "Wave rollouts analyzed between waves on error rate, p99, and token-spend anomaly from the fleet's own telemetry, auto-rollback on breach with later waves frozen, and a Git-native judgment gate (approval committed to the config repo, not a pipeline click). Closes the build plan.",
    state: "done" as const,
  },
] as const;

const NAMED_RISKS = [
  {
    name: "kubernetes-native deployment",
    body: "CRDs, an operator, Gateway API, and a production Helm chart are the next work, not built yet. The control plane distributes to any data plane over gRPC today; a k8s-native path is not claimed.",
  },
  {
    name: "public launch",
    body: "The tracker row and the public conformance scoreboard wait on the repo going public. It stays private for now, by judgment.",
  },
  {
    name: "durable counters",
    body: "Postgres-durable spend counters are deferred. Runtime state lives in memory; Git stays the only truth, never Postgres.",
  },
  {
    name: "per-event WASM hooks",
    body: "Per-event streaming hooks await a pooling-allocator spike. on_request and on_response_end ship; per-event stays gated off behind the measured budget.",
  },
  {
    name: "GB-2",
    body: "Identity from a verified login is built, then deferred by judgment. It ships when the login story is worth turning on, not before.",
  },
] as const;



export function BuildStatus() {
  return (
    <section
      id="build"
      aria-labelledby="build-heading"
      className="py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <header className="flex max-w-3xl flex-wrap items-baseline gap-x-5 gap-y-2">
          <div>
            <p className="voice-mono text-xs text-steel-dark">
              04-build-plan.md
            </p>
            <h2
              id="build-heading"
              className="voice-display mt-3 text-[length:var(--text-section)]"
            >
              Build status
            </h2>
          </div>
          <p className="voice-mono rounded-sm border border-steel bg-panel px-2.5 py-1 text-xs text-steel-dark">
            as of 2026-08-01
          </p>
        </header>
        <p className="mt-4 max-w-2xl leading-relaxed text-steel-dark">
          Risk-ordered: the highest-risk novel claim gets validated first, and
          every phase ships something a platform team can run.
        </p>
        <div aria-hidden className="mt-2 flex items-center gap-2">
          <p className="voice-hand -rotate-2 text-lg">
            the scariest claim first
          </p>
          <HandArrow className="h-8 w-10 -scale-y-100" />
        </div>

        {/* Phase 0 — closed 1 August 2026 (docs/04-build-plan.md callout) */}
        <Reveal className="mt-12">
          <div className="border border-steel border-l-4 border-l-teal bg-teal-wash p-7 sm:p-9">
            <div className="flex flex-wrap items-center gap-3">
              <h3 className="voice-display text-2xl">Phase 0: the spikes</h3>
              <StatusChip tone="teal" label="closed 1 August 2026" />
            </div>

            <div className="mt-7 grid gap-7 lg:grid-cols-2">
              <article className="border border-steel bg-[oklch(99%_0.002_95)] p-6">
                <p className="voice-mono text-xs text-steel-dark">spike A</p>
                <h4 className="mt-1.5 text-lg font-medium tracking-tight">
                  The canonical event model
                </h4>
                <div className="mt-3 flex flex-wrap gap-2">
                  <StatusChip tone="teal" label="conformance tests green" />
                  <StatusChip tone="teal" label="chunking-invariance green" />
                  <StatusChip tone="teal" label="error bound measured" />
                </div>
                <p className="mt-4 text-sm leading-relaxed text-steel-dark">
                  OpenAI SSE, Anthropic events, and Bedrock event-stream, with
                  its real binary framing, normalize into the canonical
                  stream, for text and for streamed tool calls. Replaying any
                  fixture at chunk sizes 1, 7, 64, or whole-buffer produces
                  byte-identical event streams. And the metering error bound is
                  measured and published, from 17 machine-recorded transcripts
                  rather than authored fixtures: the chars/4 estimate lands
                  within ~±50% on all but one real stream, and its worst misses
                  are structural: tool-call scaffolding and tiny responses.
                </p>
              </article>

              <article className="border border-steel bg-[oklch(99%_0.002_95)] p-6">
                <p className="voice-mono text-xs text-steel-dark">spike B</p>
                <h4 className="mt-1.5 text-lg font-medium tracking-tight">
                  The foundation bake-off: Pingora vs agentgateway
                </h4>
                <div className="mt-3 flex flex-wrap gap-2">
                  <StatusChip tone="teal" label="decided: Pingora" />
                </div>
                <p className="mt-4 text-sm leading-relaxed text-steel-dark">
                  The same minimal streaming proxy, built two ways: once on
                  Pingora, Cloudflare&rsquo;s Rust proxy library, and once
                  embedding/extending agentgateway (Apache-2.0). The Pingora
                  arm reported positive: the response body tapped without
                  buffering, every chunk fed through the Spike A adapters
                  while identical bytes stream on to the client. The
                  agentgateway arm found the core unpublished and, decisively,
                  no internal canonical event model to inherit. The data plane
                  is built on Pingora; the fork was rejected, and the
                  agentgateway GB-4/GB-8 changes proceed as parallel upstream
                  contributions. The control plane is greenfield.
                </p>
              </article>
            </div>
          </div>
        </Reveal>

        {/* The roadmap — vertical mono timeline */}
        <Reveal className="mt-14">
          <div className="grid gap-10 lg:grid-cols-[1fr_20rem]">
            <ol aria-label="Phase roadmap" className="relative space-y-0">
              {PHASES.map((phase, i) => (
                <li
                  key={phase.id}
                  className="relative grid grid-cols-[2rem_1fr] gap-4 pb-8"
                >
                  {/* rail */}
                  {i < PHASES.length - 1 && (
                    <span
                      aria-hidden
                      className="absolute left-[0.9375rem] top-7 h-full w-px bg-steel"
                    />
                  )}
                  <span
                    aria-hidden
                    className={`relative z-10 mt-1 flex size-8 items-center justify-center rounded-full border-2 bg-atrium ${
                      phase.state === "done"
                        ? "border-teal"
                        : phase.state === "in progress"
                          ? "border-gold"
                          : "border-steel"
                    }`}
                  >
                    <span className="voice-mono text-xs text-ink">
                      {phase.id}
                    </span>
                  </span>
                  <div className="pt-1">
                    <p className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                      <span className="voice-mono text-sm font-medium text-ink">
                        phase {phase.id} · {phase.title.toLowerCase()}
                      </span>
                      <span
                        className={`voice-mono text-[0.7rem] ${
                          phase.state === "done"
                            ? "text-teal-deep"
                            : phase.state === "in progress"
                              ? "text-gold-deep"
                              : "text-steel-dark"
                        }`}
                      >
                        {phase.state === "done"
                          ? "● done"
                          : phase.state === "in progress"
                            ? "● in progress"
                            : "— ahead"}
                      </span>
                    </p>
                    <p className="mt-1 max-w-xl text-sm text-steel-dark">
                      {phase.note}
                    </p>
                  </div>
                </li>
              ))}
            </ol>

            <aside className="lg:pt-1">
              <div>
                <p className="voice-mono text-xs text-steel-dark">
                  deferred, stated plainly
                </p>
                <ul className="mt-3 space-y-4">
                  {NAMED_RISKS.map((risk) => (
                    <li key={risk.name}>
                      <p className="voice-mono text-xs font-medium text-ink">
                        {risk.name}
                      </p>
                      <p className="mt-0.5 text-xs leading-relaxed text-steel-dark">
                        {risk.body}
                      </p>
                    </li>
                  ))}
                </ul>
              </div>
            </aside>
          </div>
        </Reveal>
      </div>
    </section>
  );
}
