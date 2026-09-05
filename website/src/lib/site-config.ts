/**
 * Single source of truth for site-wide constants.
 * Site for smallgateway, a small experimental gateway project.
 *
 * Temperament: the specimen sheet. The product's argument is that it is
 * small enough to read end to end, so the site is built to be read the
 * same way: numbered, precise, and full of values you can go check.
 */
export const SITE_CONFIG = {
  name: "smallgateway",
  workingName: "smallgateway",
  tagline: "A deliberately small gateway",
  description:
    "A deliberately small, experimental gateway for LLM request forwarding, attribution, token metering, and token limits.",
  url: "https://smallgateway.vercel.app",
  repoUrl: "https://github.com/Lyqed/smallgateway",
  sisterUrl: "https://thegatewaybaseline.com",
  sisterName: "The Gateway Baseline",
  locale: "en",
} as const;

export type NavItem = {
  label: string;
  href: string;
};

/** Anchored single-page navigation, numbered like the sections. */
export const NAV_ITEMS: readonly NavItem[] = [
  { label: "Shape", href: "#shape" },
  { label: "Path", href: "#path" },
  { label: "Measured", href: "#measured" },
  { label: "Project", href: "#open" },
] as const;
