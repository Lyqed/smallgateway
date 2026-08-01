import { SITE_CONFIG } from "@/lib/site-config";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { BrushField } from "@/components/art/BrushField";
import { Astronaut } from "@/components/art/Astronaut";
import { PaintBloom, SplashArcs } from "@/components/art/PaintField";
import { AnarchyStar, SprayTag } from "@/components/art/graffiti";
import { HandCircle, RingMotif } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

const INSTRUMENT_ROW = [
  "two binaries + git",
  "GB-1..GB-9 as acceptance tests",
  "apache-2.0 (working assumption)",
] as const;

/**
 * Hero (MURAL-DIRECTION) — the loudest surface. The astronaut reaching
 * for the flower is the heart image, riding a full-color paint bloom and
 * splash arcs that bleed off every edge. The machined wall (mono kicker,
 * chips, spec) stays crisp and legible; the paint is violently present
 * around it, never under the readable text. A torn edge at the bottom
 * rips the clean panel open to color, carrying the mural into Principles.
 */
export function Hero() {
  return (
    <section
      aria-labelledby="hero-heading"
      className="skylight-band relative overflow-x-clip"
    >
      {/* full-color paint bloom, bleeding from the right, behind the art */}
      <PaintBloom
        id="hero-bloom"
        className="paint-live pointer-events-none absolute -right-40 -top-40 h-[70rem] w-[70rem] max-w-[120vw] opacity-70 sm:-right-24"
      />
      {/* splash arcs sweeping across the whole hero, ignoring the grid */}
      <SplashArcs
        id="hero-arcs"
        className="paint-live-slow pointer-events-none absolute -left-10 top-16 h-[26rem] w-[130%] opacity-70"
      />
      {/* the circular gallery, cropped by the viewport (kept, machined) */}
      <RingMotif className="pointer-events-none absolute -right-56 -top-72 size-[46rem] opacity-60 sm:-right-40 sm:size-[54rem]" />

      <div className="relative mx-auto grid w-full max-w-[80rem] gap-8 px-5 pb-28 pt-28 sm:px-8 sm:pt-36 lg:grid-cols-[1.15fr_1fr] lg:items-center lg:pb-36">
        <div className="relative">
          <p className="voice-mono text-xs text-steel-dark sm:text-sm">
            {SITE_CONFIG.workingName} · a community-built LLM gateway · phase 1
          </p>

          {/* painted wall behind the headline only, so body text never
              sits on paint (MURAL-DIRECTION non-negotiable) */}
          <div className="relative">
            <BrushField className="pointer-events-none absolute -left-56 -top-2 h-[125%] w-[26rem] max-w-[75vw] opacity-90 sm:-left-44" />
            <h1
              id="hero-heading"
              className="voice-display relative mt-6 max-w-[13ch] text-[length:var(--text-hero)]"
            >
              The Open Source Gateway
            </h1>
          </div>

          <Reveal className="mt-10 max-w-2xl">
            {/* the hero statement reads at full strength on clean ground */}
            <p className="text-lg leading-relaxed text-ink sm:text-xl">
              A gateway platform teams build, own, and{" "}
              <span className="relative inline-block whitespace-nowrap">
                answer for
                <HandCircle className="pointer-events-none absolute -left-[10%] -top-[27%] h-[154%] w-[120%]" />
              </span>
              , measured by the{" "}
              <a href={SITE_CONFIG.sisterUrl} className="link-skylight">
                Gateway Baseline
              </a>
              , in the open.
            </p>
            <p
              aria-hidden
              className="marker ml-8 mt-3 -rotate-2 text-lg text-violet sm:ml-40"
            >
              answer for it: the six-month rule
            </p>
          </Reveal>

          <div className="mt-10 flex flex-wrap items-center gap-4">
            <ButtonLink href={SITE_CONFIG.repoUrl} target="_blank" rel="noreferrer">
              Read the design docs ↗
            </ButtonLink>
            <ButtonLink href={SITE_CONFIG.sisterUrl} variant="outline">
              See the Baseline
            </ButtonLink>
          </div>

          <ul
            aria-label="Project constants"
            className="mt-16 flex flex-wrap gap-x-3 gap-y-2"
          >
            {INSTRUMENT_ROW.map((chip) => (
              <li
                key={chip}
                className="voice-mono rounded-sm border border-steel bg-panel px-3 py-1.5 text-xs text-steel-dark"
              >
                {chip}
              </li>
            ))}
          </ul>
        </div>

        {/* the heart image — astronaut reaching for the flower */}
        <div className="relative mx-auto w-full max-w-[30rem] lg:mx-0">
          <Astronaut className="paint-live pointer-events-none relative z-10 w-full drop-shadow-[6px_10px_0_oklch(from_var(--steel)_l_c_h/0.35)]" />
          {/* a sprayed color word behind the astronaut's shoulder */}
          <p
            aria-hidden
            className="spray-word absolute -left-2 top-2 -rotate-6 text-5xl sm:text-6xl"
            style={{ ["--spray" as string]: "var(--gold)" }}
          >
            reach
          </p>
          {/* the hand anarchy-star, sparse, top-right */}
          <AnarchyStar className="pointer-events-none absolute -right-2 top-0 w-16 -rotate-6 sm:w-20" />
          {/* one spray tag, low and unruly */}
          <SprayTag
            id="hero-tag"
            className="pointer-events-none absolute -bottom-2 left-0 w-40 rotate-[-4deg] opacity-90"
          />
        </div>
      </div>

      {/* torn-paper edge: the clean hero panel ripped open to color as it
          meets Principles below */}
      <div
        aria-hidden
        className="torn-top relative z-10 h-4 w-full"
        style={{ ["--torn-color" as string]: "var(--monarch)" }}
      />
    </section>
  );
}
