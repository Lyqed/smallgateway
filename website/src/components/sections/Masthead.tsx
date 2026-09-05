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
 * Project name and status, followed by the figures and their sources.
 */
export function Masthead() {
  return (
    <header className="relative overflow-x-clip border-b border-steel">
      <div className="mx-auto w-full max-w-[80rem] px-5 pb-16 pt-14 sm:px-8 sm:pb-24 sm:pt-20">
        <div className="inline-flex items-center gap-2 border border-monarch/50 bg-monarch/10 px-3 py-1.5 voice-mono text-[0.65rem] uppercase tracking-[0.18em] text-monarch">
          <span aria-hidden className="size-1.5 rounded-full bg-monarch" />
          Experimental release
        </div>
        <h1 className="voice-display mt-5 max-w-[16ch] text-[length:var(--text-hero)] leading-[0.94]">
          {SITE_CONFIG.name}
        </h1>

        <div className="mt-10 grid gap-x-12 gap-y-6 lg:grid-cols-[minmax(0,32rem)_minmax(0,1fr)]">
          <p className="text-lg leading-relaxed text-ink">
            Know who used the models. Carry that identity into provider billing
            where supported. A small gateway, with fleet configuration in Git.
          </p>
          <p className="leading-relaxed text-steel-dark">
            Inspired by working with Azure API Management across clouds.
            The gateway tracks tokens, checks attribution, and attaches billing
            tags on supported provider paths. Invoice import and reconciliation
            are still planned. Current limits are in tokens.
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
          <ButtonLink href="#shape">How it works</ButtonLink>
          <a href={`${SITE_CONFIG.repoUrl}#why-i-started-this`} className="link-skylight text-sm">
            Why I started this
          </a>
          <a href={SITE_CONFIG.sisterUrl} className="link-skylight text-sm">
            Cost-attribution comparison
          </a>
        </div>
      </div>
    </header>
  );
}
