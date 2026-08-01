import { SITE_CONFIG } from "@/lib/site-config";
import { ButtonLink } from "@/components/ui/ButtonLink";
import { Astronaut } from "@/components/art/Astronaut";
import { MonarchPlanet } from "@/components/art/MonarchPlanet";
import { PaintBloom, PaintBloomCool, SplashArcs } from "@/components/art/PaintField";
import {
  AnarchyStar,
  SprayTag,
  Drips,
  ScribbleCircle,
  CrayonUnderline,
} from "@/components/art/graffiti";

/**
 * The hype landing (no-scroll, one viewport). The whole project stated as
 * one shout: the name huge, one promise line, two CTAs, and the murals
 * loud around it. Machined-meets-handmade collision compressed onto a
 * single screen. The full multi-section site lives at /full.
 */
export default function HypePage() {
  return (
    <main className="grain relative flex h-[100svh] w-full flex-col overflow-hidden bg-atrium">
      {/* Mural layer: full-color paint bleeding across the whole screen. */}
      <PaintBloom
        id="hype-bloom-a"
        className="paint-live pointer-events-none absolute -left-48 -top-40 h-[54rem] w-[54rem] max-w-[120vw] opacity-70"
      />
      <PaintBloomCool
        id="hype-bloom-b"
        className="paint-live-slow pointer-events-none absolute -bottom-56 -right-40 h-[48rem] w-[48rem] max-w-[110vw] opacity-60"
      />
      <SplashArcs
        id="hype-arcs"
        className="paint-live pointer-events-none absolute left-0 top-1/3 h-[26rem] w-[135%] opacity-60"
      />

      {/* The astronaut, cropped by the right edge, reaching in from the corner. */}
      <Astronaut className="paint-live pointer-events-none absolute -right-16 top-4 z-0 hidden w-[30rem] opacity-90 drop-shadow-[6px_10px_0_oklch(from_var(--steel)_l_c_h/0.3)] lg:block" />
      {/* The monarch on its planet, bottom-left, cropped. */}
      <MonarchPlanet className="paint-live-slow pointer-events-none absolute -bottom-24 -left-16 z-0 aspect-square w-64 -rotate-6 opacity-90 sm:w-80" />

      {/* Scattered graffiti marks. */}
      <AnarchyStar className="pointer-events-none absolute left-[8%] top-[14%] z-10 w-14 -rotate-12 sm:w-20" />
      <SprayTag id="hype-tag" className="pointer-events-none absolute right-[10%] bottom-[16%] z-10 hidden w-40 rotate-3 md:block" />
      <Drips className="pointer-events-none absolute right-[28%] top-0 z-0 h-40 w-24 opacity-70" />

      {/* Header: wordmark + repo pill, kept crisp. */}
      <header className="relative z-20 flex items-center justify-between px-6 pt-6 sm:px-10">
        <p className="voice-mono text-sm font-medium tracking-[0.18em] text-ink">
          {SITE_CONFIG.name.toUpperCase()}
        </p>
        <p className="voice-mono text-xs text-steel-dark">phase 2 · in the open</p>
      </header>

      {/* The shout: centered, one screen. */}
      <div className="relative z-20 flex flex-1 flex-col items-start justify-center px-6 sm:px-10">
        <div className="max-w-5xl">
          <p className="voice-mono mb-6 text-sm uppercase tracking-[0.22em] text-monarch-deep">
            Build it. Own it. Answer for it.
          </p>

          <h1 className="voice-display text-[clamp(2.75rem,10vw,8rem)] font-semibold leading-[0.92] tracking-tight text-ink">
            The{" "}
            <span className="relative inline-block">
              Open
              <CrayonUnderline
                className="pointer-events-none absolute -bottom-2 left-0 h-3 w-full"
                color="var(--monarch)"
              />
            </span>{" "}
            Source
            <br />
            Gateway
          </h1>

          <p className="mt-8 max-w-2xl text-lg leading-relaxed text-ink sm:text-xl">
            <span className="relative inline-block">
              A gateway platform teams build, own, and answer for.
              <span aria-hidden className="marker absolute -right-8 -top-6 hidden rotate-6 text-lg text-violet sm:block">
                yours
              </span>
            </span>
          </p>

          <div className="mt-10 flex flex-wrap items-center gap-4">
            <ButtonLink href={SITE_CONFIG.repoUrl} variant="monarch" target="_blank" rel="noreferrer">
              Read the design docs ↗
            </ButtonLink>
            <div className="relative">
              <ButtonLink href={SITE_CONFIG.sisterUrl} variant="outline">
                The cost-attribution standard
              </ButtonLink>
              <ScribbleCircle className="pointer-events-none absolute -inset-2 h-[calc(100%+1rem)] w-[calc(100%+1rem)]" />
            </div>
          </div>
        </div>
      </div>

      {/* Footer strip: quiet, on the machined base. */}
      <footer className="relative z-20 flex items-center justify-between px-6 pb-6 sm:px-10">
        <p className="voice-mono text-xs text-steel-dark">
          Open source. Built in public.
        </p>
        <a
          href="/full"
          className="voice-mono text-xs text-skylight-deep underline decoration-steel underline-offset-4 hover:text-monarch-deep"
        >
          See the full build →
        </a>
      </footer>
    </main>
  );
}
