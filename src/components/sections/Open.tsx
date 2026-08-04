import { Monarch } from "@/components/art/Monarch";
import { HandCircle } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { SITE_CONFIG } from "@/lib/site-config";

/**
 * The close: the terms, and nothing else. After three sections about
 * the system, the last one is about the people. The ownership sentence
 * carries the section heading and the one hand-circled moment in the
 * back half of the page; the monarch lands beside it, once.
 */
export function Open() {
  return (
    <section
      id="open"
      aria-labelledby="open-heading"
      className="scroll-mt-20 border-t border-steel py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        {/* The terms. The one place on the page that is about people
            rather than about the system. */}
        <Reveal>
          <div className="relative">
            {/* Hidden on the narrowest screens: below 480px it would
                sit over the eyebrow once that label wraps. */}
            <Monarch className="pointer-events-none absolute -top-10 right-2 hidden w-20 rotate-6 min-[480px]:block sm:right-8 sm:w-28" />

            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              04 · The terms
            </p>

            {/* The circle is hidden below lg. The mark is a fixed-aspect
                ellipse, so on narrow screens, where this sentence wraps
                to three or four lines, stretching it tall enough to
                enclose them puts the stroke straight through the eyebrow
                above and the copy below. It only earns its place where
                the sentence sits on one or two lines. */}
            <div className="relative mt-6 inline-block max-w-4xl lg:px-[3%] lg:py-[4%]">
              <HandCircle className="pointer-events-none absolute -left-[23%] top-1/2 hidden h-[112%] w-[99%] -translate-y-1/2 lg:block" />
              <h2
                id="open-heading"
                className="voice-display relative text-3xl leading-[1.14] sm:text-5xl"
              >
                You answer for what you merged, for as long as it runs.
              </h2>
            </div>

            <div className="mt-10 grid max-w-4xl gap-8 sm:grid-cols-2">
              <p className="text-lg leading-relaxed text-ink">
                Six months later, when something breaks and the commit has
                your name on it, you turn up. You find it, you fix it or you
                revert it, and the write-up afterward describes the
                mechanism rather than the person. That is the whole
                contract, and the technical design exists to make it
                survivable: truth in Git is what lets you reconstruct a
                night you were not there for.
              </p>
              <p className="leading-relaxed text-steel-dark">
                Tooling has closed most of the gaps that used to separate
                engineers, which leaves standing behind your own work as
                one of the few things still worth anything. Owned by the
                people who run it, open from the first commit, with nothing
                held back behind a paywall.
              </p>
            </div>

            {/* No repository button while the repo is private: the page
                closes on the standard it is measured against, which is
                the one thing a reader can go and check today. */}
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
