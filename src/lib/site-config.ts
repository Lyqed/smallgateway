/**
 * Single source of truth for site-wide constants.
 * Site B of the Gateway Baseline / Gateway Project pair — temperament: the mural.
 */
export const SITE_CONFIG = {
  name: "The Open Source Gateway",
  // The project's working name, used in copy. Final naming is an open
  // decision — docs/04 names the collision risk.
  workingName: "The Gateway Project",
  tagline: "A gateway platform teams build, own, and answer for",
  description:
    "A community-built LLM gateway — designed from scratch, owned end to end, and measured by the Gateway Baseline, in the open. Two binaries plus Git.",
  url: "https://theopensourcegateway.com",
  repoUrl: "https://github.com/Lyqed/thegatewayproject",
  sisterUrl: "https://thegatewaybaseline.com",
  sisterName: "The Gateway Baseline",
  locale: "en",
} as const;

export type NavItem = {
  label: string;
  href: string;
};

/** Anchored single-page navigation. */
export const NAV_ITEMS: readonly NavItem[] = [
  { label: "Principles", href: "#principles" },
  { label: "Architecture", href: "#architecture" },
  { label: "Build", href: "#build" },
  { label: "Contribute", href: "#contribute" },
] as const;
