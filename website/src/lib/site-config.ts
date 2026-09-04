/**
 * Single source of truth for site-wide constants.
 * Site B of the Gateway Baseline / Gateway Project pair.
 *
 * Temperament: the specimen sheet. The product's argument is that it is
 * small enough to read end to end, so the site is built to be read the
 * same way: numbered, precise, and full of values you can go check.
 */
export const SITE_CONFIG = {
  name: "The Open Source Gateway",
  workingName: "The Gateway Project",
  tagline: "Small enough to read end to end",
  description:
    "An LLM gateway you can read in an afternoon. Two binaries and a Git repository, no database in the request path, and every number on this page measured rather than claimed.",
  url: "https://opensourcegateway.com",
  repoUrl: "https://github.com/Lyqed/thegatewayproject",
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
  { label: "Terms", href: "#open" },
] as const;
