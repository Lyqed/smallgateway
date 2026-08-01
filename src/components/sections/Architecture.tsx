import { EventStream } from "@/components/art/EventStream";
import { OrbitArc } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

const DATA_PLANE_POINTS = [
  {
    lead: "metering",
    body: "Token metering on streams: an incremental tally, reconciled against the provider's terminal usage frame.",
  },
  {
    lead: "redaction",
    body: "PII redaction on deltas, not on a buffered whole.",
  },
  {
    lead: "rewriting",
    body: "Format rewriting between provider dialects.",
  },
  {
    lead: "enforcement",
    body: "Mid-stream enforcement: a budget exhausted mid-generation cuts the stream with an operator-defined terminal event. GB-4 extended to streaming; nothing in the matrix does it.",
  },
  {
    lead: "policy",
    body: "Policy chains compose fleet → project → route → app around an explicit base marker, each level a Git directory.",
  },
  {
    lead: "extend",
    body: "CEL expressions for conditions and derivations: sandboxed, no I/O, microseconds. Signed WASM modules for real programs, not promised until the Phase-4 spike proves them on hot streaming paths.",
  },
] as const;

const CONTROL_PLANE_POINTS = [
  {
    lead: "truth",
    body: "Git as truth: scoped chains and templates compile into per-data-plane rendered snapshots. Reviewable diffs, no in-gateway templating.",
  },
  {
    lead: "sync",
    body: "An xDS-style gRPC stream to every data plane: versioned snapshots, ACK/NACK.",
  },
  {
    lead: "drift",
    body: "Drift detection and self-heal: divergence is surfaced, never silent.",
  },
  {
    lead: "fleets",
    body: "GatewaySets: label selectors × generators stamp config across heterogeneous fleets: VMs, DMZ boxes, multiple clouds, edge; not just clusters.",
  },
  {
    lead: "canaries",
    body: "Config canaries: wave rollouts by failure domain, analyzed on error rate, p99, and token-spend anomaly, with auto-rollback.",
  },
  {
    lead: "budgets",
    body: "Budget shares for fleet-wide caps, with bounded-overspend semantics published. Platform teams trust stated error bounds and distrust magic.",
  },
] as const;

function PointList({
  points,
}: {
  points: readonly { lead: string; body: string }[];
}) {
  return (
    <ul className="mt-6 space-y-4">
      {points.map((point) => (
        <li key={point.lead} className="grid grid-cols-[6.5rem_1fr] gap-3">
          <span className="voice-mono pt-0.5 text-xs text-steel-dark">
            {point.lead}
          </span>
          <span className="text-sm leading-relaxed text-steel-dark">
            {point.body}
          </span>
        </li>
      ))}
    </ul>
  );
}

/**
 * Architecture (brief §8.3) — the canonical event stream as the
 * centerpiece, then the two planes. Sourced from docs/02-architecture.md.
 */
export function Architecture() {
  return (
    <section
      id="architecture"
      aria-labelledby="architecture-heading"
      className="skylight-band relative overflow-x-clip py-[var(--space-section)]"
    >
      {/* orbit arc crossing the section corner — the reconcile loop */}
      <OrbitArc className="pointer-events-none absolute -right-24 top-10 hidden w-[26rem] lg:block" />

      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <header className="max-w-2xl">
          <p className="voice-mono text-xs text-steel-dark">
            02-architecture.md
          </p>
          <h2
            id="architecture-heading"
            className="voice-display mt-3 text-[length:var(--text-section)]"
          >
            One event model over three wire formats
          </h2>
          <p className="mt-4 leading-relaxed text-steel-dark">
            Provider adapters normalize every provider&rsquo;s wire format
            (OpenAI SSE deltas, Anthropic events, Bedrock event-stream) into
            one internal event model, flowing through the response path with
            backpressure, never buffered whole. Every capability that made
            streaming a second-class citizen becomes uniform.
          </p>
        </header>

        <Reveal className="mt-14">
          <div className="border border-steel bg-[oklch(99%_0.002_95)] px-5 py-8 sm:px-10 sm:py-10">
            <EventStream />
          </div>
        </Reveal>

        <div className="mt-8 grid gap-8 lg:grid-cols-2">
          <Reveal>
            <article className="lift-card h-full border border-steel bg-panel p-7 sm:p-9">
              <p className="voice-mono text-xs text-steel-dark">
                the data plane
              </p>
              <h3 className="voice-display mt-2 text-2xl">
                Streaming, first-class
              </h3>
              <PointList points={DATA_PLANE_POINTS} />
            </article>
          </Reveal>

          <Reveal delay={120}>
            <article className="lift-card h-full border border-steel bg-panel p-7 sm:p-9">
              <p className="voice-mono text-xs text-steel-dark">
                the control plane
              </p>
              <h3 className="voice-display mt-2 text-2xl">
                ArgoCD for gateway fleets
              </h3>
              <p className="mt-4 border-l-2 border-monarch pl-3 text-sm leading-relaxed text-ink">
                The genuinely novel product is not another gateway binary. It
                is the control plane. Nobody has &ldquo;ArgoCD for gateway
                fleets.&rdquo;
              </p>
              <PointList points={CONTROL_PLANE_POINTS} />
            </article>
          </Reveal>
        </div>
      </div>
    </section>
  );
}
