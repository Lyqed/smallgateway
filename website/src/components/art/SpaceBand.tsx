import { SplashArcs } from "@/components/art/PaintField";
import { AnarchyStar, SprayTag } from "@/components/art/graffiti";
import { Reveal } from "@/components/reveal/Reveal";

/**
 * The deep-space mural band (MURAL-DIRECTION escalation) — the one surface
 * on the page where the murals fully own the wall. Sits between
 * Architecture and Build status: a full-bleed night wall, hand-drawn
 * stars, the blue planet rising as a horizon, splash arcs burning at full
 * saturation against the dark. The single statement is the project's own
 * metaphor, on clean dark ground, never on paint. Inline SVG only.
 */

function StarField({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 1200 600"
      fill="none"
      className={className}
      preserveAspectRatio="xMidYMid slice"
    >
      <defs>
        <filter id="sb-star-f" x="-10%" y="-10%" width="120%" height="120%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.9"
            numOctaves="1"
            seed="7"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="2.5"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>
      <g filter="url(#sb-star-f)" stroke="oklch(92% 0.02 95)" strokeLinecap="round">
        {/* hand-flicked dots */}
        <g strokeWidth="3" opacity="0.55">
          <path d="M80 90 l0.01 0 M210 200 l0.01 0 M330 60 l0.01 0 M470 170 l0.01 0 M600 80 l0.01 0 M720 210 l0.01 0 M860 110 l0.01 0 M980 190 l0.01 0 M1120 70 l0.01 0 M160 320 l0.01 0 M540 300 l0.01 0 M910 320 l0.01 0 M1060 280 l0.01 0" />
        </g>
        <g strokeWidth="2" opacity="0.35">
          <path d="M130 150 l0.01 0 M290 260 l0.01 0 M410 110 l0.01 0 M560 220 l0.01 0 M690 140 l0.01 0 M810 60 l0.01 0 M950 250 l0.01 0 M1160 160 l0.01 0 M60 260 l0.01 0 M380 340 l0.01 0 M760 330 l0.01 0 M1120 360 l0.01 0" />
        </g>
        {/* a few four-point sparkles, crayon-wobbled */}
        <g strokeWidth="2.5" opacity="0.6">
          <path d="M250 120 v-14 M250 120 v14 M250 120 h-14 M250 120 h14" />
          <path d="M640 260 v-11 M640 260 v11 M640 260 h-11 M640 260 h11" />
          <path d="M1040 120 v-13 M1040 120 v13 M1040 120 h-13 M1040 120 h13" />
        </g>
        {/* one shooting star, gold */}
        <path
          d="M900 60 C 830 90, 770 120, 720 150"
          stroke="var(--gold)"
          strokeWidth="3"
          opacity="0.7"
        />
      </g>
    </svg>
  );
}

function PlanetHorizon({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 1440 320"
      fill="none"
      className={className}
      preserveAspectRatio="xMidYMax slice"
    >
      <defs>
        <radialGradient id="sb-planet" cx="50%" cy="105%" r="95%">
          <stop offset="0%" stopColor="oklch(52% 0.17 262)" />
          <stop offset="55%" stopColor="oklch(38% 0.165 266)" />
          <stop offset="100%" stopColor="oklch(26% 0.13 270)" />
        </radialGradient>
        <filter id="sb-crayon" x="-10%" y="-15%" width="120%" height="130%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.6"
            numOctaves="1"
            seed="5"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="4"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>
      <g filter="url(#sb-crayon)">
        {/* the rising planet: a huge circle whose top arc is the horizon */}
        <circle cx="720" cy="1180" r="1020" fill="url(#sb-planet)" />
        {/* atmosphere glow along the horizon line */}
        <path
          d="M-20 208 C 340 130, 1100 130, 1460 208"
          stroke="oklch(72% 0.12 230)"
          strokeWidth="5"
          strokeLinecap="round"
          opacity="0.55"
          fill="none"
        />
        {/* surface scribble bands, hand-worked */}
        <path
          d="M240 280 C 460 240, 900 236, 1200 272 M 420 310 C 640 282, 980 280, 1260 306 M 90 300 C 200 276, 320 264, 460 262"
          stroke="oklch(60% 0.14 258)"
          strokeWidth="4"
          strokeLinecap="round"
          opacity="0.5"
          fill="none"
        />
        {/* craters */}
        <circle cx="560" cy="290" r="12" fill="oklch(30% 0.13 268)" opacity="0.6" />
        <circle cx="1010" cy="272" r="8" fill="oklch(30% 0.13 268)" opacity="0.55" />
        {/* orbit ring skimming the horizon */}
        <ellipse
          cx="720"
          cy="212"
          rx="560"
          ry="44"
          fill="none"
          stroke="var(--violet)"
          strokeWidth="3.5"
          opacity="0.45"
          transform="rotate(-3 720 212)"
        />
        {/* a small planted flag: the community's claim on the ground */}
        <g stroke="oklch(92% 0.02 95)" strokeWidth="3" strokeLinecap="round">
          <path d="M1130 246 v-46" />
          <path d="M1130 200 l30 8 -30 9" fill="var(--monarch)" stroke="var(--monarch)" />
        </g>
      </g>
    </svg>
  );
}

export function SpaceBand() {
  return (
    <section
      aria-label="The mural wall"
      className="relative overflow-hidden bg-[oklch(16.5%_0.05_270)]"
    >
      {/* the light machined wall above rips open into space: the page's
          own paper, torn, hanging over the dark */}
      <div
        aria-hidden
        className="torn-top absolute inset-x-0 top-4 z-10"
        style={{ ["--torn-color" as string]: "var(--surface-atrium)" }}
      />

      {/* stars behind everything */}
      <StarField className="pointer-events-none absolute inset-0 h-full w-full opacity-90" />

      {/* splash arcs at full saturation — neon against the night wall,
          bleeding in from the light section above */}
      {/* the arcs' per-path opacities are tuned for the white atrium and
          go olive over the night wall; run the paint at full ink here */}
      <SplashArcs
        id="space-arcs"
        className="feather-y paint-live pointer-events-none absolute -top-10 left-0 h-[22rem] w-[135%] opacity-90 [&_path]:opacity-90"
      />

      <div className="relative mx-auto w-full max-w-[80rem] px-5 pb-64 pt-24 sm:px-8 sm:pb-80 sm:pt-32">
        <Reveal className="relative max-w-3xl">
          {/* the project's own metaphor, stated once, on clean dark ground */}
          <p className="voice-mono text-xs uppercase tracking-[0.25em] text-[oklch(70%_0.06_250)]">
            the wall
          </p>
          <p className="voice-display mt-6 text-4xl text-[oklch(96%_0.006_95)] sm:text-6xl">
            A community of humans, building industrial infrastructure.
          </p>
          <p
            aria-hidden
            className="marker ml-6 mt-6 rotate-[-2deg] text-xl text-[oklch(76%_0.13_300)] sm:ml-16 sm:text-2xl"
          >
            machined walls, painted by hand
          </p>
        </Reveal>

        {/* graffiti punctuation — sparse, unruly */}
        <p
          aria-hidden
          className="spray-word absolute right-2 top-16 rotate-6 text-6xl sm:right-16 sm:text-7xl"
          style={{ ["--spray" as string]: "var(--gold)" }}
        >
          ours
        </p>
        <AnarchyStar className="pointer-events-none absolute right-8 bottom-40 w-14 rotate-12 opacity-90 sm:right-40 sm:w-16" />
        <SprayTag
          id="space-tag"
          className="pointer-events-none absolute -left-2 bottom-32 w-44 rotate-[-5deg] opacity-90"
        />
      </div>

      {/* the blue planet rising along the bottom edge */}
      <PlanetHorizon className="pointer-events-none absolute inset-x-0 bottom-0 h-56 w-full sm:h-72" />
    </section>
  );
}
