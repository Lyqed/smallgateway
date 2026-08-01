import { HandArrow } from "@/components/art/marks";
import { PaintBloomCool, SplashArcs } from "@/components/art/PaintField";
import { AnarchyStar, ScribbleCircle } from "@/components/art/graffiti";
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
    note: "throwaway code, non-throwaway conclusions; both spikes reported; closed 1 August 2026",
    state: "done" as const,
  },
  {
    id: "1",
    title: "Standalone data plane",
    note: "Baseline-conformant from a static file; GB-1 through GB-4, GB-7, and GB-8 pass as automated conformance tests against a real gatewayd; scoped policy chain, CEL, and hot-reload in crates/",
    state: "done" as const,
  },
  {
    id: "2",
    title: "Control plane MVP",
    note: "Two milestones landed and adversarially verified: fleet distribution (rendered snapshots over gRPC, join-token auth, all-or-nothing waves) and Git-as-truth with drift self-heal and config-PR admission. A node's drain semantics carry over the network unchanged. Multi-wave and GatewaySets remain.",
    state: "in progress" as const,
  },
  {
    id: "3",
    title: "The stateful layer",
    note: "budget shares, alerts firing from the enforcement layer itself, mid-stream enforcement wired to the shares",
    state: "ahead" as const,
  },
  {
    id: "4",
    title: "WASM SDK and hot swap",
    note: "signed modules, GB-9 with full doc-03 semantics, break-glass with TTL",
    state: "ahead" as const,
  },
  {
    id: "5",
    title: "Fleet ergonomics",
    note: "GatewaySets, tenancy scoping, config canaries with Git-native judgment gates",
    state: "ahead" as const,
  },
] as const;

const NAMED_RISKS = [
  {
    name: "the name",
    body: "“The Gateway Project” collides hard with Kubernetes Gateway API mindshare. Renaming stays cheap while the repo is private, which it remains; the decision is open and taken before public launch.",
  },
  {
    name: "WASM on the hot path",
    body: "Per-event hooks on streaming paths need real performance validation before they are promised. That is why they sit in Phase 4, behind a spike, not in the pitch.",
  },
  {
    name: "“good enough”",
    body: "The strongest competitor is not any gateway. It is “agentgateway CRDs + ArgoCD is good enough.” The win is the domain-aware reconciler and the non-k8s fleet story; a phase that advances neither is scope creep.",
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
              as of 2026-08-01
            </p>
            {/* the hand rings the date — precision, circled by the mural */}
            <ScribbleCircle
              className="pointer-events-none absolute -inset-x-3 -inset-y-2 h-[calc(100%+1rem)] w-[calc(100%+1.5rem)]"
              color="var(--violet)"
            />
          </span>
        </header>
        <p className="mt-4 max-w-2xl leading-relaxed text-steel-dark">
          Risk-ordered: the highest-risk novel claim gets validated first, and
          every phase ships something a platform team can run.
        </p>
        <div aria-hidden className="mt-2 flex items-center gap-2">
          <p className="marker -rotate-2 text-xl text-monarch">
            the scariest claim first
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
                            : "· ahead"}
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
              <p className="voice-mono text-xs text-steel-dark">legend</p>
              <ul className="voice-mono mt-3 space-y-2 text-xs text-steel-dark">
                <li className="flex items-center gap-2">
                  <span aria-hidden className="size-2 rounded-full bg-teal" />
                  done · phases 0 and 1
                </li>
                <li className="flex items-center gap-2">
                  <span aria-hidden className="size-2 rounded-full bg-gold" />
                  in progress
                </li>
                <li className="flex items-center gap-2">
                  <span
                    aria-hidden
                    className="h-px w-2 bg-steel-dark"
                  />
                  ahead · steel, not promises
                </li>
              </ul>

              <div className="mt-10 border-t border-steel pt-6">
                <p className="voice-mono text-xs text-steel-dark">
                  named risks, carried openly
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
