type WordmarkProps = {
  className?: string;
};

/**
 * The favicon stretched wide: the same gradient swash and the same
 * orange dot, drawn long instead of square. It sits above the headline
 * as a rule that happens to be the mark, so the page opens on the
 * identity without spending a line of copy on it.
 *
 * Aspect ratio is preserved rather than stretched. `none` would let the
 * swash span any width, but it would also squash the dot into an
 * ellipse and thin the stroke unevenly, so the viewBox is simply drawn
 * long and the whole mark scales as one piece.
 *
 * The gradient id is namespaced because the favicon uses `wm-tick` too,
 * and two SVGs sharing a def id on one page is a coin flip over which
 * one wins.
 */
export function Wordmark({ className }: WordmarkProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 960 26"
      fill="none"
      className={className}
    >
      <defs>
        <linearGradient id="wm-rule" x1="0" y1="1" x2="1" y2="0">
          <stop offset="0%" stopColor="#7d3bc7" />
          <stop offset="55%" stopColor="#e07a2c" />
          <stop offset="100%" stopColor="#e8b84a" />
        </linearGradient>
      </defs>

      {/* The swash: the favicon's curve pulled out to full width, still
          rising left to right and still easing off at the end. */}
      <path
        d="M4 21 C 260 16, 560 9, 900 6"
        stroke="url(#wm-rule)"
        strokeWidth="3"
        strokeLinecap="round"
      />

      {/* The dot, kept at the swash's high end where the favicon has it. */}
      <circle cx="934" cy="6" r="5" fill="#e07a2c" />
    </svg>
  );
}
