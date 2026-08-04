import { Reveal } from "@/components/reveal/Reveal";

/**
 * Monolith band (brief §4 stillness, §8.4b) — the Kubrick register.
 * Floor-dark, functionally empty, one axiom from the spec centered on
 * both axes, and the site's single watching dot below the sentence.
 * Strict one-point symmetry; no paint, no links, no second sentence.
 * Fades in slowly (1200ms, linear) — the sentence does not translate,
 * it only appears.
 */
export function MonolithBand() {
  return (
    <div className="bg-floor">
      <Reveal className="reveal-still flex min-h-[92svh] flex-col items-center justify-center gap-12 px-6 py-28 text-center">
        <p className="voice-mono text-sm tracking-[0.14em] text-atrium sm:text-base">
          It runs while you sleep.
        </p>
        <span
          aria-hidden
          className="watching-dot block size-2.5 rounded-full bg-monarch"
        />
      </Reveal>
    </div>
  );
}
