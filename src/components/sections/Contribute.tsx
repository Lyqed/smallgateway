import { SITE_CONFIG } from "@/lib/site-config";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { SparkMark } from "@/components/art/marks";
import { Reveal } from "@/components/reveal/Reveal";

const STARTING_POINTS = [
  {
    step: "01",
    title: "Read the design docs",
    body: "Every decision is written down. The reading order runs from the operating principles and design questions through architecture, hot swap, the build plan, the feature catalog, and prior art.",
  },
  {
    step: "02",
    title: "Run the spikes",
    body: "Spike A replays three providers' wire formats through the canonical event stream: cargo test, then replay the fixtures through the CLI. Spike B was the foundation bake-off that chose Pingora.",
  },
  {
    step: "03",
    title: "Adapters as first contributions",
    body: "Each provider's wire format meets the canonical stream in one adapter: a bounded, testable surface, shaped for a first PR.",
  },
] as const;

/**
 * Contribute (brief §8.6) — where to start. Violet is the human layer:
 * annotations, community, contribution.
 */
export function Contribute() {
  return (
    <section
      id="contribute"
      aria-labelledby="contribute-heading"
      className="py-[var(--space-section)] pt-0"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <Reveal>
          <div className="relative border border-violet/30 bg-violet-wash p-7 sm:p-12">
            <SparkMark
              className="pointer-events-none absolute right-6 top-6 w-6 sm:right-10 sm:top-10"
            />
            <p className="voice-mono text-xs text-steel-dark">start here</p>
            <h2
              id="contribute-heading"
              className="voice-display mt-3 text-[length:var(--text-section)]"
            >
              Contribute
            </h2>
            <p className="mt-4 max-w-2xl leading-relaxed text-steel-dark">
              A community solution, built in the open, under the ownership
              contract. The front door is the repo.
            </p>

            <ol className="mt-10 grid gap-x-10 gap-y-8 md:grid-cols-3">
              {STARTING_POINTS.map((point) => (
                <li key={point.step}>
                  <p className="voice-mono text-xs text-violet-deep">
                    {point.step}
                  </p>
                  <h3 className="mt-2 text-lg font-medium tracking-tight">
                    {point.title}
                  </h3>
                  <p className="mt-2 text-sm leading-relaxed text-steel-dark">
                    {point.body}
                  </p>
                </li>
              ))}
            </ol>

            <div className="mt-12 flex flex-wrap items-center gap-x-6 gap-y-4">
              <ButtonLink
                href={SITE_CONFIG.repoUrl}
                target="_blank"
                rel="noreferrer"
              >
                Read the design docs ↗
              </ButtonLink>
              <a href={SITE_CONFIG.sisterUrl} className="link-skylight text-sm">
                Hold it against the Baseline ↗
              </a>
            </div>

            <p className="voice-mono mt-8 max-w-2xl border-t border-violet/20 pt-5 text-xs leading-relaxed text-steel-dark">
              license: Apache-2.0 is the working assumption, to be fixed before
              the repo goes public. Deferred until then, in keeping with the
              defer-by-default rule.
            </p>
          </div>
        </Reveal>
      </div>
    </section>
  );
}
