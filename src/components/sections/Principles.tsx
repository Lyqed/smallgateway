import { ThreeBoxes } from "@/components/art/ThreeBoxes";
import { HandArrow, HandUnderline } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

/**
 * Principles (brief §8.2) — three walls, sourced from
 * docs/00-principles.md: same claims, tightened copy.
 */
export function Principles() {
  return (
    <section
      id="principles"
      aria-labelledby="principles-heading"
      className="relative py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <header className="max-w-2xl">
          <p className="voice-mono text-xs text-steel-dark">00-principles.md</p>
          <h2
            id="principles-heading"
            className="voice-display mt-3 text-[length:var(--text-section)]"
          >
            The constraints come first
          </h2>
          <p className="mt-4 text-steel-dark">
            Architecture is what survives them. Three walls the whole design
            leans on.
          </p>
        </header>

        {/* Wall 1 — wide, with the hand-sketched system beside it */}
        <Reveal className="mt-14">
          <article className="lift-card grid gap-8 border border-steel bg-panel p-7 sm:p-10 lg:grid-cols-[5fr_6fr] lg:items-center">
            <div>
              <p className="voice-mono text-xs text-steel-dark">01</p>
              <h3 className="voice-display mt-2 text-2xl sm:text-3xl">
                Two binaries plus Git
              </h3>
              <p className="mt-4 leading-relaxed text-steel-dark">
                Taken straight from the ArgoCD-vs-Spinnaker lesson: ArgoCD won
                on operational weight. Adopted here as a hard budget: one data
                plane, a single binary a team can run on a VM, in a container,
                or in a cluster, configured from a static file with no control
                plane at all; and one control plane, a single binary with
                Postgres for <em>runtime state</em>, never for truth. Truth
                lives in Git, always.
              </p>
              <p className="voice-mono mt-5 border-l-2 border-steel pl-3 text-xs text-steel-dark">
                anti-goal, restated at every phase: don&rsquo;t accrete
                Spinnaker
              </p>
            </div>
            <ThreeBoxes className="w-full" />
          </article>
        </Reveal>

        {/* Walls 2 + 3 — offset editorial pair, not a uniform grid */}
        <div className="mt-8 grid gap-8 lg:grid-cols-12">
          <Reveal className="lg:col-span-7">
            <article className="lift-card relative h-full border border-steel bg-panel p-7 sm:p-10">
              <p className="voice-mono text-xs text-steel-dark">02</p>
              <h3 className="voice-display mt-2 text-2xl sm:text-3xl">
                Defer, defer, defer
              </h3>
              <p className="mt-4 leading-relaxed text-steel-dark">
                The vendors are selling urgency; the Baseline is a list of
                eight-going-on-nine verifiable behaviors, and most of them are
                weeks of work, not platforms. So: no vendor gateway, no SaaS
                control plane, no paid observability tier until the matrix
                (verified cells, not vendor claims) shows a gap this community
                cannot close in reasonable time. Every dependency is a
                governance relationship. Reversible decisions get made fast and
                cheap; irreversible ones get made late and once.
              </p>
              <div aria-hidden className="mt-6 flex items-end gap-3">
                <HandArrow className="h-10 w-12 -scale-x-100" />
                <p className="voice-hand rotate-[2deg] pb-1 text-2xl">
                  you don&rsquo;t need to buy anything
                </p>
              </div>
            </article>
          </Reveal>

          <Reveal delay={120} className="lg:col-span-5 lg:mt-16">
            <article className="lift-card h-full border border-steel bg-panel p-7 sm:p-10">
              <p className="voice-mono text-xs text-steel-dark">03</p>
              <h3 className="voice-display mt-2 text-2xl sm:text-3xl">
                The six-month rule
              </h3>
              <p className="mt-4 leading-relaxed text-steel-dark">
                <span className="relative inline-block text-ink">
                  You own your change for its lifetime.
                  <HandUnderline className="pointer-events-none absolute -bottom-1.5 left-0 h-3 w-full" />
                </span>{" "}
                Every change is attributable and revertible: <span className="voice-mono text-sm">git log</span>{" "}
                names it, <span className="voice-mono text-sm">git revert</span>{" "}
                undoes it, and the rendered-snapshot history shows what every
                data plane was running at the moment of the incident.
                Break-glass exists, with a TTL: visible, temporary,
                auto-reverting. And stated invariants over magic: staleness
                bounds, overlap semantics, bounded-overspend windows, published
                as part of the spec.
              </p>
            </article>
          </Reveal>
        </div>
      </div>
    </section>
  );
}
