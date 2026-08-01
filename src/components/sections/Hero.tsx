import { SITE_CONFIG } from "@/lib/site-config";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { BrushField } from "@/components/art/BrushField";
import { HandCircle, RingMotif } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

const INSTRUMENT_ROW = [
  "two binaries + git",
  "GB-1..GB-9 as acceptance tests",
  "apache-2.0 — working assumption",
] as const;

/**
 * Hero (brief §8.1): brush field bleeding from the left edge behind the
 * ring; display title; two CTAs. One hand annotation — the circle
 * around "answer for", naming the six-month rule.
 */
export function Hero() {
  return (
    <section
      aria-labelledby="hero-heading"
      className="skylight-band relative overflow-x-clip"
    >
      {/* the circular gallery, cropped by the viewport */}
      <RingMotif className="pointer-events-none absolute -right-56 -top-72 size-[46rem] sm:-right-40 sm:size-[54rem]" />

      <div className="relative mx-auto w-full max-w-[80rem] px-5 pb-24 pt-28 sm:px-8 sm:pt-36 lg:pb-32">
        <p className="voice-mono text-xs text-steel-dark sm:text-sm">
          {SITE_CONFIG.workingName} · a community-built LLM gateway · phase 1
        </p>

        {/* the painted wall, bleeding in from the left edge — anchored to
            the display headline's own box so the paint tracks it at every
            breakpoint and body text never sits on paint (brief §4) */}
        <div className="relative">
          <BrushField className="pointer-events-none absolute -left-40 -top-2 h-[115%] w-[26rem] max-w-[75vw] sm:-left-28" />
          <h1
            id="hero-heading"
            className="voice-display relative mt-6 max-w-[13ch] text-[length:var(--text-hero)]"
          >
            The Open Source Gateway
          </h1>
        </div>

        <Reveal className="mt-10 max-w-2xl">
          {/* Ink: the hero statement reads at full strength; it clears the
              brush field entirely, which is anchored to the headline. */}
          <p className="text-lg leading-relaxed text-ink sm:text-xl">
            A gateway platform teams build, own, and{" "}
            <span className="relative inline-block whitespace-nowrap">
              answer for
              <HandCircle className="pointer-events-none absolute -left-[12%] -top-[38%] h-[176%] w-[124%]" />
            </span>{" "}
            — measured by the{" "}
            <a href={SITE_CONFIG.sisterUrl} className="link-skylight">
              Gateway Baseline
            </a>
            , in the open.
          </p>
          <p
            aria-hidden
            className="voice-hand ml-8 mt-3 -rotate-2 text-lg sm:ml-40"
          >
            answer for it — the six-month rule
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
    </section>
  );
}
