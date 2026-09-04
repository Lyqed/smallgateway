import type { Metadata } from "next";
import { Space_Grotesk, IBM_Plex_Mono, Shantell_Sans } from "next/font/google";
import "./globals.css";
import { SITE_CONFIG } from "@/lib/site-config";
import { SkipLink } from "@/components/layout/SkipLink";
import { SiteHeader } from "@/components/layout/SiteHeader";
import { SiteFooter } from "@/components/layout/SiteFooter";

/** The machined voice — display, headings, UI, body. */
const grotesk = Space_Grotesk({
  variable: "--font-grotesk",
  subsets: ["latin"],
  display: "swap",
});

/** The instrument voice — codes, data cells, dates, spec clauses. */
const plexMono = IBM_Plex_Mono({
  variable: "--font-plex",
  subsets: ["latin"],
  weight: ["400", "500"],
  display: "swap",
});

/** The mural voice — hand annotations only. Subset, not preloaded:
 * it is the exception the concept pays for. */
const shantell = Shantell_Sans({
  variable: "--font-shantell",
  subsets: ["latin"],
  weight: "400",
  display: "swap",
  preload: false,
});

export const metadata: Metadata = {
  metadataBase: new URL(SITE_CONFIG.url),
  // The tab reads as the name alone. A tagline appended here is
  // truncated to noise in a narrow tab and repeats what the page says
  // in its first line anyway.
  title: {
    default: "Open Source Gateway",
    template: `%s · Open Source Gateway`,
  },
  description: SITE_CONFIG.description,
  keywords: [
    "LLM gateway",
    "open source",
    "Gateway Baseline",
    "token metering",
    "streaming proxy",
    "control plane",
  ],
  openGraph: {
    type: "website",
    siteName: SITE_CONFIG.name,
    title: `${SITE_CONFIG.name} · ${SITE_CONFIG.tagline}`,
    description: SITE_CONFIG.description,
    url: SITE_CONFIG.url,
  },
  twitter: {
    card: "summary_large_image",
    title: SITE_CONFIG.name,
    description: SITE_CONFIG.description,
  },
  alternates: { canonical: "/" },
  robots: { index: true, follow: true },
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html
      lang={SITE_CONFIG.locale}
      className={`${grotesk.variable} ${plexMono.variable} ${shantell.variable} h-full`}
    >
      <body className="flex min-h-full flex-col overflow-x-clip">
        <SkipLink />
        <SiteHeader />
        <main id="main" className="flex-1">
          {children}
        </main>
        <SiteFooter />
      </body>
    </html>
  );
}
