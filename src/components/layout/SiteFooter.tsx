import { SITE_CONFIG } from "@/lib/site-config";

/**
 * The polished floor: the one dark surface on the site. Small print
 * only. The closing line states the standing invitation without asking
 * for anything, which is the whole posture of the project.
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
            <p className="voice-mono mt-4 text-xs leading-relaxed text-[oklch(66%_0.012_255)]">
              &ldquo;{SITE_CONFIG.workingName}&rdquo; is a working name. The
              final one is still an open decision, carried in the open like
              everything else here.
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
                <a
                  href={SITE_CONFIG.repoUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="link-floor inline-block py-1.5"
                >
                  github.com/Lyqed/thegatewayproject ↗
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
