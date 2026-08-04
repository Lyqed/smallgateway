import { SITE_CONFIG } from "@/lib/site-config";

/**
 * The polished floor: the one dark surface on the site. Two links and
 * two lines of small print. The repository is reachable from the header
 * and from the close of the page, so it is not repeated here.
 */
export function SiteFooter() {
  return (
    <footer className="polished-floor mt-auto text-[oklch(88%_0.01_250)]">
      <div className="mx-auto w-full max-w-[80rem] px-5 py-16 sm:px-8">
        <div className="flex flex-col justify-between gap-10 md:flex-row md:items-end">
          <div className="max-w-md">
            <p className="voice-display text-2xl text-atrium">
              Small enough to read.
              <br />
              Open enough to check.
            </p>
          </div>

          <nav aria-label="Footer">
            <ul className="voice-mono flex flex-col gap-1 text-sm md:items-end">
              <li>
                <a href={SITE_CONFIG.sisterUrl} className="link-floor inline-block py-1.5">
                  {SITE_CONFIG.sisterName} ↗
                </a>
              </li>
              <li>
                <a href="#main" className="link-floor inline-block py-1.5">
                  Back to top
                </a>
              </li>
            </ul>
          </nav>
        </div>

        <div className="mt-12 flex flex-col gap-2 border-t border-floor-soft pt-6 sm:flex-row sm:items-center sm:justify-between">
          <p className="voice-mono text-xs text-[oklch(66%_0.012_255)]">
            {SITE_CONFIG.name}
          </p>
          <p className="voice-mono text-xs text-[oklch(66%_0.012_255)]">
            Nothing here is for sale.
          </p>
        </div>
      </div>
    </footer>
  );
}
