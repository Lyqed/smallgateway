import { NAV_ITEMS, SITE_CONFIG } from "@/lib/site-config";

/**
 * Anchored single-page nav. Server component — plain fragment links,
 * no client JS. The wordmark carries the site's one persistent mark:
 * a small ring crossed by a brush tick.
 */
export function SiteHeader() {
  return (
    <header className="sticky top-0 z-50 border-b border-steel/60 bg-atrium">
      <div className="mx-auto flex min-h-14 w-full max-w-[80rem] flex-wrap items-center justify-between gap-x-6 gap-y-1 px-5 py-2 sm:px-8">
        <a
          href="#main"
          className="flex items-center gap-2.5 text-sm font-medium tracking-tight text-ink"
        >
          <svg aria-hidden viewBox="0 0 28 28" className="size-6 shrink-0">
            <defs>
              <linearGradient id="wm-tick" x1="0" y1="1" x2="1" y2="0">
                <stop offset="0%" stopColor="var(--violet)" />
                <stop offset="55%" stopColor="var(--monarch)" />
                <stop offset="100%" stopColor="var(--gold)" />
              </linearGradient>
            </defs>
            <circle
              cx="14"
              cy="14"
              r="11"
              fill="none"
              stroke="var(--ink)"
              strokeWidth="2"
            />
            {/* the brush tick, escalated to a full-color paint stroke */}
            <path
              d="M4 21 C 10 15, 19 10, 26 8"
              fill="none"
              stroke="url(#wm-tick)"
              strokeWidth="3.5"
              strokeLinecap="round"
            />
            {/* a small sprayed color dot — the mural, contained */}
            <circle cx="22" cy="9" r="2" fill="var(--monarch)" />
          </svg>
          {SITE_CONFIG.name}
        </a>

        <nav aria-label="Section navigation" className="flex items-center">
          <ul className="voice-mono flex flex-wrap items-center gap-x-5 gap-y-1 text-[0.8rem]">
            {NAV_ITEMS.map((item) => (
              <li key={item.href}>
                <a
                  href={item.href}
                  className="text-steel-dark transition-colors duration-150 hover:text-ink"
                >
                  {item.label}
                </a>
              </li>
            ))}
            <li>
              <a
                href={SITE_CONFIG.repoUrl}
                target="_blank"
                rel="noreferrer"
                className="text-ink underline decoration-monarch decoration-2 underline-offset-4 transition-[text-decoration-thickness] duration-150 hover:decoration-[3px]"
              >
                GitHub ↗
              </a>
            </li>
          </ul>
        </nav>
      </div>
    </header>
  );
}
