import { Reveal } from "@/components/reveal/Reveal";
import { SHAPE } from "@/lib/specimen";

/**
 * The two binaries, set as an asymmetric pair rather than two equal
 * cards: the data plane is the one that must exist, so it gets the
 * weight, and the control plane is presented honestly as optional. The
 * standalone badge is the argument, not decoration.
 */
export function Shape() {
  return (
    <section
      id="shape"
      aria-labelledby="shape-heading"
      className="scroll-mt-20 py-[var(--space-section)]"
    >
      <div className="mx-auto w-full max-w-[80rem] px-5 sm:px-8">
        <div className="grid gap-x-12 gap-y-5 lg:grid-cols-[minmax(0,26rem)_minmax(0,1fr)]">
          <div>
            <p className="voice-mono text-xs uppercase tracking-[0.28em] text-steel-dark">
              01 · Shape
            </p>
            <h2
              id="shape-heading"
              className="voice-display mt-4 text-[length:var(--text-section)] leading-tight"
            >
              Two binaries, and you may only want one
            </h2>
          </div>
          <p className="max-w-2xl self-end leading-relaxed text-steel-dark">
            Start with a gateway and a configuration file. The optional
            control plane distributes configuration from Git when you have
            several instances to manage.
          </p>
        </div>

        <div className="mt-16 flex flex-col gap-px bg-steel">
          {SHAPE.map((piece, i) => (
            <Reveal key={piece.name} delay={i * 90}>
              <article className="grid gap-x-10 gap-y-5 bg-atrium py-10 sm:grid-cols-[5rem_minmax(0,1fr)] sm:py-12 lg:grid-cols-[5rem_minmax(0,20rem)_minmax(0,1fr)]">
                <p className="voice-mono text-sm text-steel-dark">
                  {piece.index}
                </p>

                <div>
                  <h3 className="voice-display text-2xl leading-tight sm:text-3xl">
                    {piece.name}
                  </h3>
                  <p className="mt-2 text-sm leading-relaxed text-steel-dark">
                    {piece.role}
                  </p>
                  {piece.standalone && (
                    <p className="voice-mono mt-5 inline-flex border border-teal-deep/40 bg-teal-wash px-2.5 py-1 text-[0.65rem] uppercase tracking-[0.16em] text-teal-deep">
                      runs alone
                    </p>
                  )}
                </div>

                <p className="max-w-2xl text-lg leading-relaxed text-ink">
                  {piece.detail}
                </p>
              </article>
            </Reveal>
          ))}
        </div>

        <Reveal>
          <p className="mt-12 max-w-3xl border-l-2 border-monarch pl-5 text-lg leading-relaxed text-ink">
            The standalone gateway needs no control plane. With fleet
            management enabled, gateways retain their current configuration
            if that connection drops. Shared token limits have separate
            partition behavior, described in the deployment notes.
          </p>
        </Reveal>
      </div>
    </section>
  );
}
