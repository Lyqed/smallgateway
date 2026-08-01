import { Monarch } from "@/components/art/Monarch";
import { HandCircle } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

/**
 * The ownership contract (brief §8.5) — the mural moment. The butterfly
 * lands here, once per site, at the moment of transformation. Half
 * machined type, the key phrase circled by hand.
 */
export function Ownership() {
  return (
    <section
      aria-labelledby="ownership-heading"
      className="relative overflow-x-clip pb-[var(--space-section)] pt-[clamp(7rem,42svh,26rem)]"
    >
      {/* The generous top gap is deliberate: the monolith band above must
          never share a viewport with this section's mural moment. */}
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <div className="relative border-y border-steel py-16 sm:py-20">
          <Monarch className="pointer-events-none absolute -bottom-12 right-2 w-24 rotate-6 sm:-bottom-14 sm:right-10 sm:w-32" />

          <p className="voice-mono text-sm font-medium uppercase tracking-[0.2em] text-monarch sm:text-base">
            The ownership contract
          </p>

          <Reveal className="mt-8">
            {/* The hand circle wraps the WHOLE sentence, not one phrase:
                a large ellipse sized to the heading's bounding box, drawn
                behind the multi-line text. */}
            <div className="relative inline-block max-w-4xl px-[3%] py-[6%]">
              <HandCircle className="pointer-events-none absolute inset-0 h-full w-full" />
              <h2
                id="ownership-heading"
                className="voice-display relative text-3xl leading-[1.12] sm:text-5xl"
              >
                If something breaks because of a change you made six months ago,
                you are responsible.
              </h2>
            </div>
            <p aria-hidden className="voice-hand mt-6 rotate-[-2deg] text-xl">
              not blamed. responsible
            </p>
          </Reveal>

          <div className="mt-10 grid gap-8 lg:grid-cols-2">
            <p className="leading-relaxed text-steel-dark">
              You show up, you diagnose, you fix or revert, and the postmortem
              names the mechanism, not the person. This is the community&rsquo;s
              contract, and it shapes the technical design directly: Git as
              truth is what makes the six-month rule survivable.
            </p>
            <p className="leading-relaxed text-steel-dark">
              What that buys a community: the tooling closes the skill gap, so
              the differentiator is knowing
              how to build, collaborate, and stand behind changes for their
              whole lifetime. A gateway owned end to end, by the people who run
              it.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}
