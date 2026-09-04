import { Monarch } from "./Monarch";

type MonarchPlanetProps = {
  className?: string;
};

/**
 * The monarch on a deep-blue planet (MURAL-DIRECTION §1), escalated from
 * the base Monarch. The butterfly rides a deep-blue world with a ringed
 * orbit and color-splash arcs bleeding out of frame. Used once, at the
 * ownership moment. Inline SVG, aria-hidden, no external assets.
 */
export function MonarchPlanet({ className }: MonarchPlanetProps) {
  return (
    <div aria-hidden className={className}>
      <svg
        viewBox="0 0 440 440"
        fill="none"
        className="absolute inset-0 h-full w-full"
        preserveAspectRatio="xMidYMid meet"
      >
        <defs>
          <radialGradient id="mp-planet" cx="40%" cy="36%" r="78%">
            <stop offset="0%" stopColor="oklch(48% 0.16 265)" />
            <stop offset="58%" stopColor="oklch(34% 0.16 268)" />
            <stop offset="100%" stopColor="oklch(21% 0.12 270)" />
          </radialGradient>
          <linearGradient id="mp-arc" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0%" stopColor="var(--gold)" />
            <stop offset="50%" stopColor="var(--monarch)" />
            <stop offset="100%" stopColor="var(--violet)" />
          </linearGradient>
          <filter id="mp-crayon" x="-15%" y="-15%" width="130%" height="130%">
            <feTurbulence
              type="fractalNoise"
              baseFrequency="0.85"
              numOctaves="1"
              seed="4"
              result="n"
            />
            <feDisplacementMap
              in="SourceGraphic"
              in2="n"
              scale="3"
              xChannelSelector="R"
              yChannelSelector="G"
            />
          </filter>
        </defs>

        {/* color-splash arcs, bleeding off frame */}
        <g filter="url(#mp-crayon)" opacity="0.85">
          <path
            d="M-30 300 C 90 160, 280 80, 470 70"
            stroke="url(#mp-arc)"
            strokeWidth="34"
            strokeLinecap="round"
            opacity="0.5"
          />
          <path
            d="M-20 360 C 110 240, 300 168, 470 158"
            stroke="var(--teal)"
            strokeWidth="12"
            strokeLinecap="round"
            opacity="0.5"
          />
        </g>

        {/* the deep-blue planet */}
        <g filter="url(#mp-crayon)">
          <circle cx="220" cy="248" r="150" fill="url(#mp-planet)" />
          {/* orbit ring, crayon-wobbled, crossing behind the planet */}
          <ellipse
            cx="220"
            cy="248"
            rx="196"
            ry="64"
            fill="none"
            stroke="var(--violet)"
            strokeWidth="4"
            opacity="0.55"
            transform="rotate(-22 220 248)"
          />
          {/* surface scribble bands */}
          <path
            d="M96 236 C 150 226, 236 230, 340 250 M 110 296 C 170 288, 250 292, 336 310 M 130 192 C 190 186, 262 190, 322 204"
            stroke="oklch(58% 0.14 262)"
            strokeWidth="4"
            strokeLinecap="round"
            opacity="0.5"
            fill="none"
          />
          <circle cx="176" cy="220" r="14" fill="oklch(28% 0.12 270)" opacity="0.55" />
          <circle cx="272" cy="300" r="9" fill="oklch(28% 0.12 270)" opacity="0.5" />
        </g>
      </svg>

      {/* the butterfly, riding the planet, drawn by the escalated Monarch */}
      <Monarch className="absolute left-1/2 top-[18%] w-[46%] -translate-x-1/2" />
    </div>
  );
}
