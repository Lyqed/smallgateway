import { MonarchPlanet } from "@/components/art/MonarchPlanet";
import { PaintBloom, SplashArcs } from "@/components/art/PaintField";
import { CrayonUnderline } from "@/components/art/graffiti";
import { HandCircle } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

/**
 * The ownership contract (MURAL-DIRECTION) — the mural climax. The monarch
 * lands here on a deep-blue planet, once per site, at the moment of
 * transformation, ringed by splash arcs and a full-color bloom that bleed
 * off the edges. The contract sentence stays on clean ground, circled by
 * hand; the body copy sits on the atrium, never on paint. Contrast holds.
 */
export function Ownership() {
  return (
    <section
      aria-labelledby="ownership-heading"
      className="relative overflow-x-clip pb-[var(--space-section)] pt-[clamp(7rem,42svh,26rem)]"
    >
      {/* The generous top gap keeps the dark build band out of this section's
          viewport. Paint bleeds up into that gap and off both edges. */}
      <PaintBloom
        id="own-bloom"
        className="paint-live pointer-events-none absolute -left-40 top-0 h-[52rem] w-[52rem] max-w-[110vw] opacity-60"
      />
      <SplashArcs
        id="own-arcs"
        className="paint-live-slow pointer-events-none absolute -right-10 top-24 h-[24rem] w-[125%] opacity-60"
      />

      <div className="relative mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <div className="relative border-y border-steel py-16 sm:py-20">
          {/* the monarch on its deep-blue planet — the once-per-site mural */}
          <MonarchPlanet className="pointer-events-none absolute -bottom-24 right-0 aspect-square w-56 rotate-6 sm:-bottom-28 sm:right-6 sm:w-72" />

          <div className="relative inline-block">
            <p className="voice-mono text-sm font-medium uppercase tracking-[0.2em] text-monarch-deep sm:text-base">
              The ownership contract
            </p>
            <CrayonUnderline
              className="pointer-events-none absolute -bottom-2 left-0 h-3 w-full"
              color="var(--monarch)"
            />
          </div>

          <Reveal className="mt-10">
            {/* The hand circle wraps the whole sentence; the sentence sits on
                the clean atrium ground behind the paint, at full contrast. */}
            <div className="relative inline-block max-w-4xl px-[3%] py-[6%]">
              <HandCircle className="pointer-events-none absolute -left-[12%] top-1/2 h-[118%] w-[114%] -translate-y-1/2" />
              <h2
                id="ownership-heading"
                className="voice-display relative text-3xl leading-[1.12] sm:text-5xl"
              >
                If something breaks because of a change you made six months ago,
                you are responsible.
              </h2>
            </div>
            <p aria-hidden className="marker mt-6 rotate-[-2deg] text-2xl text-violet">
              not blamed — responsible.
            </p>
          </Reveal>

          <div className="mt-10 grid max-w-4xl gap-8 lg:grid-cols-2">
            <p className="leading-relaxed text-steel-dark">
              You show up, you diagnose, you fix or revert, and the postmortem
              names the mechanism, not the person. This is the community&rsquo;s
              contract, and it shapes the technical design directly: Git as
              truth is what makes the six-month rule survivable.
            </p>
            <p className="leading-relaxed text-steel-dark">
              What that buys a community: the tooling closes the skill gap, so
              the only edge left is standing behind your changes for their whole
              life. A gateway owned end to end by the people who run it, and by
              no one else. Nobody profits from it. No consultancy, no paid tier,
              no upsell. It is the community&rsquo;s, and it stays that way.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}
