import { SITE_CONFIG } from "@/lib/site-config";

/**
 * The polished floor (brief §4): the one dark surface on the site.
 * Dark band with a faint vertical reflection of the content above,
 * mono small print, sister-site and repo links.
 */
export function SiteFooter() {
  return (
    <footer className="polished-floor mt-auto text-[oklch(88%_0.01_250)]">
      <div className="mx-auto w-full max-w-[80rem] px-5 py-16 sm:px-8">
        <div className="flex flex-col justify-between gap-10 md:flex-row md:items-end">
          <div className="max-w-md">
            <p className="voice-display text-2xl text-atrium">
              Built in the open.
              <br />
              Measured by the Baseline.
            </p>
            <p className="voice-mono mt-4 text-xs leading-relaxed text-[oklch(66%_0.012_255)]">
              &ldquo;{SITE_CONFIG.workingName}&rdquo; is the working name. The
              final name is an open decision, carried openly.
            </p>
          </div>

          <nav aria-label="Footer">
            <ul className="voice-mono flex flex-col gap-2.5 text-sm md:items-end">
              <li>
                <a href={SITE_CONFIG.sisterUrl} className="link-floor">
                  {SITE_CONFIG.sisterName} · the yardstick ↗
                </a>
              </li>
              <li>
                <a
                  href={SITE_CONFIG.repoUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="link-floor"
                >
                  github.com/Lyqed/thegatewayproject ↗
                </a>
              </li>
              <li>
                <a href="#main" className="link-floor">
                  Back to top
                </a>
              </li>
            </ul>
          </nav>
        </div>

        <div className="mt-12 flex flex-col gap-2 border-t border-floor-soft pt-6 sm:flex-row sm:items-center sm:justify-between">
          <p className="voice-mono text-xs text-[oklch(66%_0.012_255)]">
            {SITE_CONFIG.name} · sister site of{" "}
            <a href={SITE_CONFIG.sisterUrl} className="link-floor">
              thegatewaybaseline.com
            </a>
          </p>
          <p className="voice-mono text-xs text-[oklch(66%_0.012_255)]">
            You don&rsquo;t need to buy anything. Yet.
          </p>
        </div>
      </div>
    </footer>
  );
}
