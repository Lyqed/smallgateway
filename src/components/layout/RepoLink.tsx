"use client";

/**
 * The repository link while the repository is private.
 *
 * It is a real link, focusable and keyboard-operable, but it reloads
 * the page instead of navigating. The distinction matters: `href="/"`
 * is a fresh navigation and lands the reader at the top, and `href="#"`
 * smooth-scrolls there because scroll-behavior is smooth on html.
 * `location.reload()` is a reload, and browsers restore scroll position
 * across a reload, so the reader stays where they were.
 *
 * href is kept as the current path so the control still reads as a link
 * to assistive tech and still works with keyboard activation. The
 * default navigation is prevented; the reload is what runs.
 */
export function RepoLink({ className }: { className?: string }) {
  return (
    <a
      href="/"
      className={className}
      onClick={(event) => {
        event.preventDefault();
        window.location.reload();
      }}
    >
      GitHub ↗
    </a>
  );
}
