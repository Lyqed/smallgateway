import { HandCircle } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { SITE_CONFIG } from "@/lib/site-config";
import { HEADLINE_FIGURES } from "@/lib/specimen";

const KIND_LABEL: Record<string, string> = {
  measured: "measured",
  bounded: "bounded",
  chosen: "chosen",
};

/**
 * The masthead reads as the header of a specimen sheet rather than a
 * landing hero: the claim, then immediately four figures with their
 * provenance. Nothing here asks for a signup. The only thing circled by
 * hand is the phrase the whole project is staked on.
 */
export function Masthead() {
  return (
    <header className="relative overflow-x-clip border-b border-steel">
      <div className="mx-auto w-full max-w-[80rem] px-5 pb-16 pt-20 sm:px-8 sm:pb-24 sm:pt-28">
        <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
          {SITE_CONFIG.workingName}
        </p>

        <h1 className="voice-display mt-6 max-w-[22ch] text-[length:var(--text-hero)] leading-[0.94]">
          An LLM gateway you can{" "}
          <span className="relative inline-block whitespace-nowrap">
            read
            <HandCircle className="pointer-events-none absolute -left-[9%] top-1/2 h-[150%] w-[118%] -translate-y-1/2" />
          </span>{" "}
          end to end
        </h1>

        <div className="mt-10 grid gap-x-12 gap-y-6 lg:grid-cols-[minmax(0,32rem)_minmax(0,1fr)]">
          <p className="text-lg leading-relaxed text-ink">
            Most infrastructure asks for trust because nobody has time to
            audit it. This one is built the other way around: two binaries
            and a Git repository, sized so that one engineer can hold the
            whole thing in their head on a long afternoon.
          </p>
          <p className="leading-relaxed text-steel-dark">
            Every figure below says where it came from. The ones marked
            measured were written down by a run rather than by a person.
            The ones marked bounded are limits the code refuses to cross.
            The ones marked chosen are decisions, with the reasoning
            written down somewhere you can go and disagree with it.
          </p>
        </div>

        <Reveal className="mt-14">
          <dl className="grid grid-cols-1 border-t border-steel sm:grid-cols-2 lg:grid-cols-4">
            {HEADLINE_FIGURES.map((figure, i) => (
              <div
                key={figure.label}
                className={`border-b border-steel px-0 py-7 sm:px-7 ${
                  i === 0 ? "sm:pl-0" : ""
                } ${i > 0 ? "sm:border-l" : ""}`}
              >
                {/* The term is what is being quantified, the description
                    is its value, and the term is written first so the
                    markup reads in the order the spec requires. Visual
                    order is reversed with flex, which puts the big number
                    on top without lying about the structure. Provenance
                    is tied to the pair by aria-describedby rather than
                    stuffed inside it. */}
                <p className="voice-mono text-[0.65rem] uppercase tracking-[0.2em] text-steel-dark">
                  {KIND_LABEL[figure.kind]}
                </p>
                <div className="mt-3 flex flex-col-reverse">
                  <dt
                    className="mt-3 text-sm font-medium leading-snug text-ink"
                    aria-describedby={`fig-src-${i}`}
                  >
                    {figure.label}
                  </dt>
                  <dd className="voice-display ml-0 text-5xl leading-none text-ink">
                    {figure.value}
                  </dd>
                </div>
                <p
                  id={`fig-src-${i}`}
                  className="mt-3 text-[0.82rem] leading-relaxed text-steel-dark"
                >
                  {figure.source}
                </p>
              </div>
            ))}
          </dl>
        </Reveal>

        <div className="mt-12 flex flex-wrap items-center gap-x-6 gap-y-4">
          <ButtonLink href="#shape">Read the shape of it</ButtonLink>
          <a href={SITE_CONFIG.sisterUrl} className="link-skylight text-sm">
            The standard it is measured against
          </a>
        </div>
      </div>
    </header>
  );
}
