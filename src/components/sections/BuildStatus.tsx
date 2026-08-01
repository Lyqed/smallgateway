import { HandArrow } from "@/components/art/marks";
import { PaintBloomCool, SplashArcs } from "@/components/art/PaintField";
import { AnarchyStar, ScribbleCircle } from "@/components/art/graffiti";
import { Reveal } from "@/components/reveal/Reveal";

/** Honest, dated status, sourced from docs/04-build-plan.md, the phase
 * READMEs, and the repo as of 2026-08-02. All six phases are done and
 * adversarially verified. No invented progress, and no withheld progress
 * either; the deferred items are stated plainly, not hidden. */

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

const DEFERRED = [
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
      className="relative overflow-x-clip py-[var(--space-section)]"
    >
      {/* paint bleeds behind the header and margins only; every fact-bearing
          surface below keeps its clean/wash ground so no mural touches a
          number, chip, or phase state (MURAL-DIRECTION non-negotiable) */}
      <PaintBloomCool
        id="build-bloom"
        className="paint-live pointer-events-none absolute -right-48 -top-24 h-[46rem] w-[46rem] max-w-[100vw] opacity-40"
      />
      <SplashArcs
        id="build-arcs"
        className="paint-live-slow pointer-events-none absolute -left-16 top-8 h-[18rem] w-[120%] opacity-45"
      />

      <div className="relative mx-auto w-full max-w-[80rem] px-5 sm:px-8">
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
          <span className="relative inline-block">
            <p className="voice-mono rounded-sm border border-steel bg-panel px-2.5 py-1 text-xs text-steel-dark">
              as of 2026-08-02
            </p>
            {/* the hand rings the date — precision, circled by the mural */}
            <ScribbleCircle
              className="pointer-events-none absolute -inset-x-3 -inset-y-2 h-[calc(100%+1rem)] w-[calc(100%+1.5rem)]"
              color="var(--violet)"
            />
          </span>
        </header>
        <p className="mt-4 max-w-2xl leading-relaxed text-steel-dark">
          All six phases are done, built in risk order so the scariest novel
          claim was validated first, and each was verified by an adversarial
          critique that caught real defects a demo would have missed. Two
          binaries plus Git, with runtime state in memory and Git as the only
          source of truth.
        </p>
        <div aria-hidden className="mt-2 flex items-center gap-2">
          <p className="marker -rotate-2 text-xl text-monarch">
            the whole plan, closed
          </p>
          <HandArrow className="h-8 w-10 -scale-y-100" />
          <AnarchyStar className="w-9 -rotate-6" />
        </div>

        {/* Phase 0 — closed 1 August 2026 (docs/04-build-plan.md callout).
            Torn open to teal, but the well and its cards keep clean grounds
            so every chip and the measured error bound stay fully legible. */}
        <Reveal className="mt-12">
          <div
            className="torn-top relative border border-steel border-l-4 border-l-teal bg-teal-wash p-7 sm:p-9"
            style={{ ["--torn-color" as string]: "var(--teal)" }}
          >
            <div className="flex flex-wrap items-center gap-3">
              <h3 className="voice-display text-2xl">Phase 0 · the spikes</h3>
              <StatusChip tone="teal" label="closed · 1 August 2026" />
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
                  <StatusChip tone="teal" label="decided · Pingora" />
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

        {/* The record — six phases wheatpasted on the wall as torn
            newsprint clippings. Each clipping keeps its clean paper ground
            (facts never sit on paint); the tears, tape, and stamps carry
            the collage energy. */}
        <Reveal className="mt-16">
          {/* masthead */}
          <div className="border-y-2 border-ink py-2">
            <p className="voice-mono text-center text-[0.7rem] uppercase tracking-[0.3em] text-ink">
              the build record · six phases · each verified by adversarial
              critique
            </p>
          </div>

          <ol
            aria-label="Phase record"
            className="mt-12 grid gap-x-6 gap-y-12 sm:grid-cols-2 lg:grid-cols-3"
          >
            {PHASES.map((phase, i) => {
              const tear = ["clip-tear-a", "clip-tear-b", "clip-tear-c"][i % 3];
              const tilt = ["-rotate-1", "rotate-[0.75deg]", "rotate-[-0.5deg]", "rotate-1", "rotate-[-1.25deg]", "rotate-[0.5deg]"][i];
              const drop = ["", "sm:translate-y-4", "sm:-translate-y-2", "sm:translate-y-2", "", "sm:translate-y-3"][i];
              return (
                <li key={phase.id} className={`relative ${tilt} ${drop}`}>
                  {/* tape holding the clipping to the wall */}
                  <span
                    aria-hidden
                    className="tape left-1/2 top-0 z-10 h-6 w-20 -translate-x-1/2 -translate-y-1/2 rotate-[-3deg]"
                  />
                  <div className="tear-shadow h-full">
                    <article className={`newsprint ${tear} flex h-full flex-col p-6 pb-7`}>
                      {/* dateline */}
                      <p className="voice-mono border-b border-ink/60 pb-2 text-[0.65rem] uppercase tracking-[0.18em] text-steel-dark">
                        phase {phase.id} · closed · august 2026
                      </p>
                      <h4 className="voice-print mt-3 text-2xl font-bold leading-tight">
                        {phase.title}
                      </h4>
                      <p className="voice-print mt-3 text-[0.9rem] leading-relaxed text-ink/80">
                        {phase.note}
                      </p>
                      {/* the verification, stamped */}
                      <div className="mt-auto flex justify-end pt-5">
                        <span className="stamp -rotate-6 text-[0.65rem] text-teal-deep">
                          verified
                        </span>
                      </div>
                    </article>
                  </div>
                </li>
              );
            })}
          </ol>

          {/* classifieds: the deferred items, stated plainly, small print */}
          <aside
            aria-label="Deferred work"
            className="relative mx-auto mt-16 max-w-3xl rotate-[0.5deg]"
          >
            <span
              aria-hidden
              className="tape -top-3 left-8 z-10 h-6 w-16 rotate-[4deg]"
            />
            <span
              aria-hidden
              className="tape -top-3 right-10 z-10 h-6 w-16 rotate-[-5deg]"
            />
            <div className="tear-shadow">
              <div className="newsprint clip-tear-b p-6 sm:p-8">
                <p className="voice-mono border-b-2 border-ink pb-2 text-center text-[0.7rem] uppercase tracking-[0.3em] text-ink">
                  classifieds · deferred, stated plainly
                </p>
                <ul className="mt-5 columns-1 gap-8 sm:columns-2 [&>li]:break-inside-avoid">
                  {DEFERRED.map((item) => (
                    <li key={item.name} className="mb-5">
                      <p className="voice-mono text-xs font-semibold uppercase tracking-wide text-ink">
                        {item.name}
                      </p>
                      <p className="voice-print mt-1 text-[0.85rem] leading-relaxed text-ink/75">
                        {item.body}
                      </p>
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          </aside>
        </Reveal>
      </div>
    </section>
  );
}
