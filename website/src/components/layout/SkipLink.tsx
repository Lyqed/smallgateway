/**
 * Keyboard skip link. Visually hidden until focused, then anchored
 * top-left so keyboard users can jump past the nav to main content.
 */
export function SkipLink() {
  return (
    <a
      href="#main"
      className="sr-only focus:not-sr-only focus:fixed focus:left-4 focus:top-4 focus:z-[100] focus:rounded-sm focus:bg-monarch focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-floor"
    >
      Skip to content
    </a>
  );
}
