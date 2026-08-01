/**
 * Painterly full-color gradient fields and color-splash arcs
 * (MURAL-DIRECTION §1) — bigger and far more saturated than the original
 * BrushField, built to bleed across section boundaries and ignore the
 * grid. All inline SVG, aria-hidden, themeable via mural tokens. No
 * external assets. Every field carries a crayon/turbulence filter so the
 * paint reads hand-worked, and the `paint-live*` class (added by the
 * caller) makes it drift unless reduced-motion is set.
 */

type FieldProps = {
  className?: string;
  /** unique per instance — SVG gradient/filter ids are document-global */
  id: string;
};

/** A large multi-tone paint bloom: gold → monarch → violet, bleeding. */
export function PaintBloom({ className, id }: FieldProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 800 800"
      fill="none"
      className={className}
      preserveAspectRatio="xMidYMid slice"
    >
      <defs>
        <radialGradient id={`${id}-a`} cx="42%" cy="40%" r="65%">
          <stop offset="0%" stopColor="var(--gold)" stopOpacity="0.9" />
          <stop offset="55%" stopColor="var(--monarch)" stopOpacity="0.7" />
          <stop offset="100%" stopColor="var(--monarch)" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${id}-b`} cx="60%" cy="62%" r="60%">
          <stop offset="0%" stopColor="var(--violet)" stopOpacity="0.85" />
          <stop offset="70%" stopColor="var(--violet)" stopOpacity="0" />
        </radialGradient>
        <filter id={`${id}-f`} x="-20%" y="-20%" width="140%" height="140%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.012 0.018"
            numOctaves="2"
            seed="11"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="60"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>
      <g filter={`url(#${id}-f)`}>
        <path
          d="M120 160 C 320 60, 560 90, 660 260 C 740 400, 700 560, 520 640 C 320 726, 120 680, 80 500 C 48 356, 40 240, 120 160 Z"
          fill={`url(#${id}-a)`}
        />
        <path
          d="M420 300 C 560 260, 700 330, 700 470 C 700 600, 560 660, 440 620 C 340 588, 300 470, 340 380 C 360 336, 388 312, 420 300 Z"
          fill={`url(#${id}-b)`}
        />
      </g>
    </svg>
  );
}

/** A teal + skylight cool bloom for the machined/verified zones. */
export function PaintBloomCool({ className, id }: FieldProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 800 800"
      fill="none"
      className={className}
      preserveAspectRatio="xMidYMid slice"
    >
      <defs>
        <radialGradient id={`${id}-a`} cx="46%" cy="44%" r="64%">
          <stop offset="0%" stopColor="var(--teal)" stopOpacity="0.85" />
          <stop offset="60%" stopColor="var(--skylight)" stopOpacity="0.5" />
          <stop offset="100%" stopColor="var(--skylight)" stopOpacity="0" />
        </radialGradient>
        <filter id={`${id}-f`} x="-20%" y="-20%" width="140%" height="140%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.013 0.02"
            numOctaves="2"
            seed="6"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="54"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>
      <g filter={`url(#${id}-f)`}>
        <path
          d="M160 200 C 360 100, 600 140, 660 320 C 710 470, 620 620, 440 660 C 260 700, 120 620, 100 440 C 84 320, 96 268, 160 200 Z"
          fill={`url(#${id}-a)`}
        />
      </g>
    </svg>
  );
}

/**
 * Color-splash arcs — gold, violet, teal strokes that sweep across and
 * bleed off the edge, ignoring the section box. The whole arc bundle is
 * one wide diagonal gesture; caller positions it to cross a boundary.
 */
export function SplashArcs({ className, id }: FieldProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 1200 400"
      fill="none"
      className={className}
      preserveAspectRatio="none"
    >
      <defs>
        <filter id={`${id}-f`} x="-10%" y="-40%" width="120%" height="180%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.006 0.02"
            numOctaves="2"
            seed="3"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="26"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>
      <g filter={`url(#${id}-f)`} strokeLinecap="round" fill="none">
        <path
          d="M-40 300 C 260 140, 620 120, 1240 60"
          stroke="var(--gold)"
          strokeWidth="30"
          opacity="0.55"
        />
        <path
          d="M-40 340 C 300 200, 660 176, 1240 120"
          stroke="var(--monarch)"
          strokeWidth="16"
          opacity="0.6"
        />
        <path
          d="M-40 250 C 300 120, 700 96, 1240 30"
          stroke="var(--violet)"
          strokeWidth="12"
          opacity="0.55"
        />
        <path
          d="M-40 380 C 320 260, 720 232, 1240 190"
          stroke="var(--teal)"
          strokeWidth="9"
          opacity="0.5"
        />
        {/* drips falling off the lowest arc */}
        <path
          d="M300 268 C 300 300, 302 328, 298 352 M 620 214 C 620 250, 622 284, 618 312 M 900 190 C 900 230, 902 262, 898 292"
          stroke="var(--monarch)"
          strokeWidth="6"
          opacity="0.5"
        />
      </g>
    </svg>
  );
}
