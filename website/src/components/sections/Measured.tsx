import { HandArrow } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";
import { MEASURED } from "@/lib/specimen";

/**
 * Claim, method, and the uncomfortable part, in three columns. The third
 * column is why the section exists: anyone can publish claims, and the
 * cheapest way to be trustworthy is to publish what is wrong with them
 * before somebody else does.
 */
export function Measured() {
  return (
    <section
      id="measured"
      aria-labelledby="measured-heading"
      className="scroll-mt-20 py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <div className="grid gap-x-12 gap-y-5 lg:grid-cols-[minmax(0,26rem)_minmax(0,1fr)]">
          <div>
            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              03 · Measured
            </p>
            <h2
              id="measured-heading"
              className="voice-display mt-4 text-[length:var(--text-section)] leading-tight"
            >
              Four claims, and what is wrong with each
            </h2>
          </div>
          <div className="max-w-2xl self-end">
            <p className="leading-relaxed text-steel-dark">
              A claim with no stated failure mode has usually not been
              tested hard enough to have found one. Each row here carries
              the method that produced it and the part that still does not
              work, in the same weight of type.
            </p>
            <div aria-hidden className="mt-5 flex items-center gap-3">
              <HandArrow className="h-8 w-10 -scale-y-100" />
              <p className="voice-hand -rotate-2 text-lg text-violet">
                the third column is the point
              </p>
            </div>
          </div>
        </div>

        <div className="mt-16 border-t border-steel">
          {MEASURED.map((row, i) => (
            <Reveal key={row.claim} delay={i * 60}>
              <article className="grid gap-x-10 gap-y-6 border-b border-steel py-10 lg:grid-cols-3 lg:py-12">
                <div>
                  <p className="voice-mono text-[0.65rem] uppercase tracking-[0.2em] text-steel-dark">
                    claim
                  </p>
                  <h3 className="mt-3 text-lg font-medium leading-snug text-ink">
                    {row.claim}
                  </h3>
                </div>

                <div>
                  <p className="voice-mono text-[0.65rem] uppercase tracking-[0.2em] text-steel-dark">
                    how it was checked
                  </p>
                  <p className="mt-3 leading-relaxed text-steel-dark">
                    {row.method}
                  </p>
                </div>

                <div className="border-l-2 border-gold pl-5">
                  <p className="voice-mono text-[0.65rem] uppercase tracking-[0.2em] text-gold-deep">
                    what is still wrong
                  </p>
                  <p className="mt-3 leading-relaxed text-ink">{row.caveat}</p>
                </div>
              </article>
            </Reveal>
          ))}
        </div>
      </div>
    </section>
  );
}
