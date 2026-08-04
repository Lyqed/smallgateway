import { Monarch } from "@/components/art/Monarch";
import { HandCircle } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { SITE_CONFIG } from "@/lib/site-config";
import { NOT_YET } from "@/lib/specimen";

/**
 * The close: what is missing, then the terms. Deliberately in that
 * order. The ownership sentence is the one hand-circled moment in the
 * back half of the page, and the monarch lands beside it, once.
 */
export function Open() {
  return (
    <section
      id="open"
      aria-labelledby="open-heading"
      className="scroll-mt-20 border-t border-steel py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <div className="grid gap-x-12 gap-y-5 lg:grid-cols-[minmax(0,26rem)_minmax(0,1fr)]">
          <div>
            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              04 · Open
            </p>
            <h2
              id="open-heading"
              className="voice-display mt-4 text-[length:var(--text-section)] leading-tight"
            >
              What is not built
            </h2>
          </div>
          <p className="max-w-2xl self-end leading-relaxed text-steel-dark">
            Listed before anyone has to ask, because the gap between what a
            project says it does and what it does is the only thing that
            costs a reader real time.
          </p>
        </div>

        <ul className="mt-14 grid gap-x-10 gap-y-10 sm:grid-cols-2">
          {NOT_YET.map((item, i) => (
            <Reveal key={item.name} delay={i * 60}>
              <li className="h-full border-t-2 border-steel pt-5">
                <p className="voice-mono text-sm font-medium text-ink">
                  {item.name}
                </p>
                <p className="mt-3 leading-relaxed text-steel-dark">
                  {item.body}
                </p>
              </li>
            </Reveal>
          ))}
        </ul>

        {/* The terms. The one place on the page that is about people
            rather than about the system. */}
        <Reveal>
          <div className="relative mt-24 border-t border-steel pt-16 sm:mt-28">
            {/* Hidden on the narrowest screens: below 480px it would
                sit over the eyebrow once that label wraps. */}
            <Monarch className="pointer-events-none absolute -top-10 right-2 hidden w-20 rotate-6 min-[480px]:block sm:right-8 sm:w-28" />

            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              the terms
            </p>

            <div className="relative mt-6 inline-block max-w-4xl px-[3%] py-[5%]">
              <HandCircle className="pointer-events-none absolute -left-[10%] top-1/2 h-[124%] w-[116%] -translate-y-1/2" />
              <p className="voice-display relative text-3xl leading-[1.14] sm:text-5xl">
                You answer for what you merged, for as long as it runs.
              </p>
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

            <div className="mt-12 flex flex-wrap items-center gap-x-6 gap-y-4">
              <ButtonLink href={SITE_CONFIG.repoUrl}>
                The repository
              </ButtonLink>
              <a
                href={SITE_CONFIG.sisterUrl}
                className="link-skylight text-sm"
              >
                {SITE_CONFIG.sisterName}
              </a>
            </div>
          </div>
        </Reveal>
      </div>
    </section>
  );
}
