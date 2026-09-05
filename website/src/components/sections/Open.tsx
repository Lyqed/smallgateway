import { Monarch } from "@/components/art/Monarch";
import { HandCircle } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { SITE_CONFIG } from "@/lib/site-config";

/**
 * The close: current scope and ways to contribute. The monarch and
 * hand-drawn circle keep the same layout as the technical sections.
 */
export function Open() {
  return (
    <section
      id="open"
      aria-labelledby="open-heading"
      className="scroll-mt-20 border-t border-steel py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        {/* Project scope and contribution notes. */}
        <Reveal>
          <div className="relative">
            {/* Hidden on the narrowest screens: below 480px it would
                sit over the eyebrow once that label wraps. */}
            <Monarch className="pointer-events-none absolute -top-10 right-2 hidden w-20 rotate-6 min-[480px]:block sm:right-8 sm:w-28" />

            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              04 · The project
            </p>

            {/* The circle is hidden below lg. The mark is a fixed-aspect
                ellipse, so on narrow screens, where this sentence wraps
                to three or four lines, stretching it tall enough to
                enclose them puts the stroke straight through the eyebrow
                above and the copy below. It only earns its place where
                the sentence sits on one or two lines. */}
            <div className="relative mt-6 inline-block max-w-4xl lg:px-[3%] lg:py-[4%]">
              <HandCircle className="pointer-events-none absolute -left-[10%] top-1/2 hidden h-[112%] w-[99%] -translate-y-1/2 lg:block" />
              <h2
                id="open-heading"
                className="voice-display relative text-3xl leading-[1.14] sm:text-5xl"
              >
                A small project, with room for small fixes.
              </h2>
            </div>

            <div className="mt-10 grid max-w-4xl gap-8 sm:grid-cols-2">
              <p className="text-lg leading-relaxed text-ink">
                The focus is forwarding model requests, identifying who
                made them, recording token usage, and applying token limits.
                Provider billing tags are included where the API supports
                them. Invoice reconciliation and dollar allocation sit
                outside this project.
              </p>
              <p className="leading-relaxed text-steel-dark">
                This is an experimental project by Anton Braverman, with
                no stable release or production support commitment yet.
                Bug reports, small fixes, and clearer examples are welcome.
                The Gateway Baseline is a related comparison of attribution
                features.
              </p>
            </div>

            <div className="mt-12 flex flex-wrap items-center gap-x-6 gap-y-4">
              <ButtonLink href={SITE_CONFIG.sisterUrl}>
                {SITE_CONFIG.sisterName}
              </ButtonLink>
            </div>
          </div>
        </Reveal>
      </div>
    </section>
  );
}
