type WordmarkProps = {
  className?: string;
};

/**
 * The favicon at its own proportions: the gradient swash with the
 * orange dot at its high end. Drawn compact rather than stretched
 * across the page, so it reads as a mark instead of as a stray rule.
 *
 * The gradient id is namespaced because the favicon uses `wm-tick` too,
 * and two SVGs sharing a def id on one page is a coin flip over which
 * one wins.
 */
export function Wordmark({ className }: WordmarkProps) {
  return (
    <svg aria-hidden viewBox="0 0 64 26" fill="none" className={className}>
      <defs>
        <linearGradient id="wm-rule" x1="0" y1="1" x2="1" y2="0">
          <stop offset="0%" stopColor="#7d3bc7" />
          <stop offset="55%" stopColor="#e07a2c" />
          <stop offset="100%" stopColor="#e8b84a" />
        </linearGradient>
      </defs>

      <path
        d="M3 21 C 16 16, 34 8, 50 5"
        stroke="url(#wm-rule)"
        strokeWidth="3.5"
        strokeLinecap="round"
      />
      <circle cx="58" cy="5.5" r="4.5" fill="#e07a2c" />
    </svg>
  );
}
