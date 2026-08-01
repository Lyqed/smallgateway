/**
 * The recurring visual vocabulary (brief §4), built once and reused.
 * Everything here is decorative-with-a-referent: each mark is placed
 * next to the real thing it annotates, and every SVG is aria-hidden —
 * the information always lives in the adjacent text.
 */

type MarkProps = {
  className?: string;
};

/** Hand-wobbled ellipse for circling a word. Draws itself on scroll. */
export function HandCircle({ className }: MarkProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 220 90"
      fill="none"
      className={className}
      preserveAspectRatio="none"
    >
      <path
        className="draw-path"
        pathLength={1}
        d="M118 12 C 62 6, 14 22, 12 46 C 10 71, 62 84, 116 82 C 172 80, 210 64, 208 42 C 206 20, 158 8, 104 11 C 88 12, 74 15, 63 19"
        stroke="var(--violet)"
        strokeWidth="2.5"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

/** Underline swash — a hand stroke beneath a phrase. */
export function HandUnderline({ className }: MarkProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 240 22"
      fill="none"
      className={className}
      preserveAspectRatio="none"
    >
      <path
        className="draw-path"
        pathLength={1}
        d="M4 14 C 48 8, 118 6, 178 9 C 202 10, 222 12, 236 15 M 30 18 C 70 14, 120 13, 158 14"
        stroke="var(--violet)"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** Hand connector arrow, pointing from a note to its subject. */
export function HandArrow({ className }: MarkProps) {
  return (
    <svg aria-hidden viewBox="0 0 90 70" fill="none" className={className}>
      <path
        className="draw-path"
        pathLength={1}
        d="M84 6 C 66 28, 44 46, 14 58 M 26 44 L 12 59 L 32 62"
        stroke="var(--violet)"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** Tiny 4-point hand star. Sparse by rule. */
export function SparkMark({ className }: MarkProps) {
  return (
    <svg aria-hidden viewBox="0 0 40 40" fill="none" className={className}>
      <path
        d="M20 3 C 21 12, 22 15, 24 18 C 30 19, 34 20, 37 21 C 30 23, 26 24, 23 26 C 22 31, 21 34, 20 38 C 18 32, 17 28, 16 25 C 11 23, 7 22, 3 20 C 10 18, 14 17, 17 15 C 18 11, 19 8, 20 3 Z"
        fill="var(--gold)"
      />
    </svg>
  );
}

/** Thin hand-wobbled orbit arc with a small planet dot. */
export function OrbitArc({ className }: MarkProps) {
  return (
    <svg aria-hidden viewBox="0 0 420 200" fill="none" className={className}>
      <path
        className="draw-path"
        pathLength={1}
        d="M8 188 C 60 96, 168 22, 288 14 C 342 11, 392 26, 414 52"
        stroke="var(--violet)"
        strokeWidth="2"
        strokeLinecap="round"
        opacity="0.8"
      />
      <circle cx="288" cy="14" r="7" fill="var(--steel)" />
    </svg>
  );
}

/** Large circular gallery outline, cropped by the viewport. */
export function RingMotif({ className }: MarkProps) {
  return (
    <svg aria-hidden viewBox="0 0 800 800" fill="none" className={className}>
      <circle
        cx="400"
        cy="400"
        r="396"
        stroke="var(--steel)"
        strokeWidth="1.5"
      />
      <circle
        cx="400"
        cy="400"
        r="330"
        stroke="var(--steel)"
        strokeWidth="1"
        opacity="0.5"
      />
    </svg>
  );
}

/** Perforated rail — the mesh railing as a dot-grid divider, ≤6% opacity.
 * `id` must be unique per instance (SVG pattern ids are document-global). */
export function PerforatedRail({
  id,
  className,
}: MarkProps & { id: string }) {
  return (
    <svg
      aria-hidden
      className={className}
      height="44"
      width="100%"
      role="presentation"
    >
      <defs>
        <pattern
          id={id}
          width="14"
          height="14"
          patternUnits="userSpaceOnUse"
        >
          <circle cx="7" cy="7" r="2.1" fill="var(--ink)" opacity="0.06" />
        </pattern>
      </defs>
      <rect width="100%" height="44" fill={`url(#${id})`} />
    </svg>
  );
}
