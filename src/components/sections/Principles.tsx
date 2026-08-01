import { ThreeBoxes } from "@/components/art/ThreeBoxes";
import { PaintBloomCool, SplashArcs } from "@/components/art/PaintField";
import {
  AnarchyStar,
  Drips,
  ScribbleCircle,
  TornStrip,
} from "@/components/art/graffiti";
import { HandArrow, HandUnderline } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

/**
 * Principles (MURAL-DIRECTION) — the three walls, now mural-hosted. A cool
 * paint bloom and splash arcs bleed across the section boundary behind the
 * machined cards; each numbered wall is a clean panel torn open to color
 * along its top edge, with graffiti punctuating exactly where the
 * engineering is most precise (the numbers, the anti-goal). Card body copy
 * stays on the clean panel ground and never on paint.
 */
export function Principles() {
  return (
    <section
      id="principles"
      aria-labelledby="principles-heading"
      className="relative overflow-x-clip py-[var(--space-section)]"
    >
      {/* paint bleeding UP across the boundary from the hero, behind cards */}
      <PaintBloomCool
        id="prin-bloom"
        className="paint-live-slow pointer-events-none absolute -left-52 -top-24 h-[54rem] w-[54rem] max-w-[110vw] opacity-55"
      />
      <SplashArcs
        id="prin-arcs"
        className="paint-live pointer-events-none absolute -right-16 top-40 h-[22rem] w-[120%] opacity-55"
      />

      <div className="relative mx-auto w-full max-w-[80rem] px-5 sm:px-8">
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

        {/* Wall 1 — wide, torn open to monarch, the sketched system beside it */}
        <Reveal className="relative mt-14">
          {/* a scrap of torn newsprint underneath the tear — the literal
              "torn newsprint underneath" reference, ripped open at the top edge */}
          <TornStrip
            id="prin-news"
            className="pointer-events-none absolute -top-6 right-10 z-0 w-44 rotate-2 opacity-90 sm:right-16 sm:w-52"
          />
          <article
            className="lift-card torn-top relative z-10 grid gap-8 border border-steel bg-panel p-7 sm:p-10 lg:grid-cols-[5fr_6fr] lg:items-center"
            style={{ ["--torn-color" as string]: "var(--monarch)" }}
          >
            <div className="relative">
              <span className="relative inline-block">
                <p className="voice-mono text-xs text-steel-dark">01</p>
                <ScribbleCircle
                  className="pointer-events-none absolute -left-3 -top-2 h-8 w-14"
                  color="var(--monarch)"
                />
              </span>
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
              <p className="voice-mono mt-5 border-l-2 border-monarch pl-3 text-xs text-steel-dark">
                anti-goal, restated at every phase: don&rsquo;t accrete
                Spinnaker
              </p>
            </div>
            <ThreeBoxes className="w-full" />
          </article>
        </Reveal>

        {/* Walls 2 + 3 — offset editorial pair, each torn to a different color */}
        <div className="mt-8 grid gap-8 lg:grid-cols-12">
          <Reveal className="lg:col-span-7">
            <article
              className="lift-card torn-top relative h-full border border-steel bg-panel p-7 sm:p-10"
              style={{ ["--torn-color" as string]: "var(--violet)" }}
            >
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
                <p className="marker rotate-[2deg] pb-1 text-2xl text-violet">
                  you don&rsquo;t need to buy anything, yet
                </p>
              </div>
              {/* drips off the torn edge, low-left */}
              <Drips
                className="pointer-events-none absolute -top-3 left-8 h-10 w-24"
                color="var(--violet)"
              />
            </article>
          </Reveal>

          <Reveal delay={120} className="lg:col-span-5 lg:mt-16">
            <article
              className="lift-card torn-top relative h-full border border-steel bg-panel p-7 sm:p-10"
              style={{ ["--torn-color" as string]: "var(--gold)" }}
            >
              <AnarchyStar className="pointer-events-none absolute -right-3 -top-6 w-14 rotate-6" />
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
