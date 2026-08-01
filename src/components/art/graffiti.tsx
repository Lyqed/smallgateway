/**
 * Graffiti vocabulary (MURAL-DIRECTION §2) — spray tags, drips, a hand
 * anarchy-style star, marker scrawl, sketchy crayon circles and
 * underlines. Sparse by rule, unruly by intent. All inline SVG,
 * aria-hidden, themeable, with SVG-turbulence spray-noise. No external
 * assets. These punctuate the machined layer; they never carry
 * information (the adjacent text always does).
 *
 * Any component taking spray needs a unique `id` — SVG filter/gradient
 * ids are document-global.
 */

type MarkProps = {
  className?: string;
};

/** Hand-drawn anarchy-style star inside a rough ring — the encircled
 * five-point star, drawn loose like a marker on a wall. */
export function AnarchyStar({ className }: MarkProps) {
  return (
    <svg aria-hidden viewBox="0 0 120 120" fill="none" className={className}>
      {/* rough encircling ring */}
      <path
        d="M60 8 C 92 8, 112 34, 112 60 C 112 92, 86 112, 60 112 C 28 112, 8 84, 10 58 C 12 30, 34 8, 62 9"
        stroke="var(--ink)"
        strokeWidth="4"
        strokeLinecap="round"
        fill="none"
      />
      {/* the five-point star, one continuous scrawled stroke */}
      <path
        d="M60 22 L 74 58 L 112 58 L 80 80 L 92 116 L 60 92 L 28 116 L 40 80 L 8 58 L 46 58 Z"
        stroke="var(--monarch)"
        strokeWidth="5"
        strokeLinejoin="round"
        strokeLinecap="round"
        fill="none"
        transform="scale(0.82) translate(13 13)"
      />
    </svg>
  );
}

/** A spray tag: a scrawled word-shaped stroke with a sprayed-noise halo.
 * Purely decorative — the halo is turbulence, the stroke is a gesture. */
export function SprayTag({ className, id }: MarkProps & { id: string }) {
  return (
    <svg aria-hidden viewBox="0 0 260 120" fill="none" className={className}>
      <defs>
        <filter id={`${id}-spray`} x="-20%" y="-20%" width="140%" height="140%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.7"
            numOctaves="2"
            seed="5"
            result="n"
          />
          <feColorMatrix in="n" type="saturate" values="0" result="g" />
          <feComponentTransfer in="g" result="mask">
            <feFuncA type="discrete" tableValues="0 0 0 0 1 0 0 1 0" />
          </feComponentTransfer>
          <feComposite in="SourceGraphic" in2="mask" operator="in" />
        </filter>
      </defs>
      {/* sprayed halo underlay */}
      <rect
        x="10"
        y="20"
        width="240"
        height="80"
        rx="30"
        fill="var(--violet)"
        opacity="0.28"
        filter={`url(#${id}-spray)`}
      />
      {/* the tag gesture — a fast marker scrawl */}
      <path
        d="M24 78 C 40 44, 64 42, 70 70 C 74 88, 90 88, 96 66 C 100 50, 116 48, 120 72 C 122 86, 136 88, 146 68 C 158 44, 182 46, 188 74 C 192 92, 214 92, 236 60"
        stroke="var(--violet)"
        strokeWidth="6"
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
      />
    </svg>
  );
}

/** Paint drips falling from an edge. Give the color via `color` (a token
 * name like "var(--monarch)"). Sparse, uneven, hand-placed. */
export function Drips({
  className,
  color = "var(--monarch)",
}: MarkProps & { color?: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 200 120"
      fill="none"
      className={className}
      preserveAspectRatio="none"
    >
      <g stroke={color} strokeLinecap="round" fill="none">
        <path d="M20 0 C 20 34, 22 58, 18 84" strokeWidth="7" opacity="0.7" />
        <path d="M70 0 C 70 22, 72 40, 68 56" strokeWidth="5" opacity="0.6" />
        <path d="M120 0 C 120 44, 122 74, 118 104" strokeWidth="8" opacity="0.7" />
        <path d="M170 0 C 170 18, 172 32, 168 46" strokeWidth="5" opacity="0.55" />
      </g>
      {/* the swelling drop-heads */}
      <circle cx="18" cy="86" r="6" fill={color} opacity="0.7" />
      <circle cx="118" cy="106" r="7" fill={color} opacity="0.7" />
    </svg>
  );
}

/** Sketchy crayon scribble-circle for ringing a number or spec clause.
 * Looser and heavier than the precise HandCircle in marks.tsx. */
export function ScribbleCircle({
  className,
  color = "var(--monarch)",
}: MarkProps & { color?: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 200 120"
      fill="none"
      className={className}
      preserveAspectRatio="none"
    >
      <path
        className="draw-path"
        pathLength={1}
        d="M110 14 C 54 8, 16 30, 14 60 C 12 90, 60 106, 108 104 C 158 102, 192 80, 188 52 C 184 24, 138 10, 88 14 C 66 16, 46 22, 34 32 C 60 20, 96 16, 132 22"
        stroke={color}
        strokeWidth="4"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
        fill="none"
      />
    </svg>
  );
}

/** Heavy crayon underline — two uneven passes, marker weight. */
export function CrayonUnderline({
  className,
  color = "var(--monarch)",
}: MarkProps & { color?: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 240 24"
      fill="none"
      className={className}
      preserveAspectRatio="none"
    >
      <path
        className="draw-path"
        pathLength={1}
        d="M6 12 C 60 6, 140 5, 234 11 M 20 19 C 80 14, 150 13, 226 17"
        stroke={color}
        strokeWidth="4"
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}

/** Torn newsprint strip — a scrap of grid/clipping showing under a tear.
 * Machined mono lines on an off-white scrap with ragged edges. */
export function TornStrip({ className, id }: MarkProps & { id: string }) {
  return (
    <svg aria-hidden viewBox="0 0 240 90" fill="none" className={className}>
      <defs>
        <pattern
          id={`${id}-news`}
          width="240"
          height="16"
          patternUnits="userSpaceOnUse"
        >
          <rect width="240" height="4" y="4" fill="var(--steel-dark)" opacity="0.35" />
        </pattern>
        <clipPath id={`${id}-clip`}>
          <path d="M4 8 L 20 4 L 44 10 L 70 3 L 96 9 L 130 4 L 164 10 L 196 4 L 224 9 L 236 6 L 234 80 L 210 86 L 176 80 L 140 86 L 104 80 L 70 86 L 38 80 L 10 85 L 6 78 Z" />
        </clipPath>
      </defs>
      <g clipPath={`url(#${id}-clip)`}>
        <rect width="240" height="90" fill="oklch(96% 0.01 90)" />
        <rect width="240" height="90" fill={`url(#${id}-news)`} />
        {/* a headline scrap, blocked out */}
        <rect x="16" y="20" width="120" height="12" fill="var(--ink)" opacity="0.55" />
        <rect x="16" y="40" width="200" height="6" fill="var(--steel-dark)" opacity="0.5" />
        <rect x="16" y="52" width="180" height="6" fill="var(--steel-dark)" opacity="0.45" />
        <rect x="16" y="64" width="140" height="6" fill="var(--steel-dark)" opacity="0.4" />
      </g>
    </svg>
  );
}
