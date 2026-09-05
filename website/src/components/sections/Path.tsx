import { EventStream } from "@/components/art/EventStream";
import { Reveal } from "@/components/reveal/Reveal";
import { PATH } from "@/lib/specimen";

/**
 * One request, in four moves. Numbered like a procedure because that is
 * what it is. Each step carries the invariant it holds in fine print
 * underneath, which is the part an engineer actually reads.
 */
export function Path() {
  return (
    <section
      id="path"
      aria-labelledby="path-heading"
      className="skylight-band scroll-mt-20 border-y border-steel py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <div className="grid gap-x-12 gap-y-5 lg:grid-cols-[minmax(0,26rem)_minmax(0,1fr)]">
          <div>
            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              02 · Path
            </p>
            <h2
              id="path-heading"
              className="voice-display mt-4 text-[length:var(--text-section)] leading-tight"
            >
              What happens to one request
            </h2>
          </div>
          <p className="max-w-2xl self-end leading-relaxed text-steel-dark">
            For supported streaming responses, the gateway reads usage
            while forwarding bytes to the caller. Token estimates help
            enforce limits before a provider sends its final usage count.
          </p>
        </div>

        <Reveal className="mt-14">
          <div className="border border-steel bg-[oklch(99%_0.002_95)] px-5 py-8 sm:px-10 sm:py-10">
            <EventStream />
          </div>
        </Reveal>

        <ol className="mt-16 grid gap-x-10 gap-y-12 sm:grid-cols-2">
          {PATH.map((step, i) => (
            <Reveal key={step.index} delay={i * 70}>
              <li className="grid h-full grid-cols-[3.5rem_minmax(0,1fr)] gap-x-4">
                <span
                  aria-hidden
                  className="voice-display text-3xl leading-none text-steel"
                >
                  {step.index}
                </span>
                <div>
                  <h3 className="text-xl font-medium leading-snug text-ink">
                    {step.title}
                  </h3>
                  <p className="mt-3 leading-relaxed text-steel-dark">
                    {step.body}
                  </p>
                  <p className="voice-mono mt-5 border-t border-steel pt-3 text-[0.72rem] leading-relaxed text-steel-dark">
                    holds: {step.holds}
                  </p>
                </div>
              </li>
            </Reveal>
          ))}
        </ol>
      </div>
    </section>
  );
}
