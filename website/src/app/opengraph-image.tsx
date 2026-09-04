import { readFile } from "node:fs/promises";
import path from "node:path";
import { ImageResponse } from "next/og";
import { SITE_CONFIG } from "@/lib/site-config";

export const size = { width: 1200, height: 630 };
export const contentType = "image/png";
export const alt = `${SITE_CONFIG.name}: ${SITE_CONFIG.tagline}`;

/** Fonts are vendored (Satori needs TTF; fetching Google Fonts at build
 * time is flaky on deploy platforms). Same two voices as the page. */
function loadFont(file: string) {
  return readFile(path.join(process.cwd(), "src/lib/og-fonts", file));
}

/**
 * OG card: brush field (the mural, violet) bleeding from the left edge
 * behind the ring, display title on the gallery-white ground.
 * Colors are sRGB approximations of the OKLCH tokens — Satori-safe.
 */
export default async function Image() {
  const [grotesk, plexMono] = await Promise.all([
    loadFont("space-grotesk-600.ttf"),
    loadFont("plex-mono-400.ttf"),
  ]);

  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "center",
          backgroundColor: "#f8f7f2",
          position: "relative",
          padding: "72px 80px",
        }}
      >
        {/* the ring, cropped by the frame */}
        <svg
          width="820"
          height="820"
          viewBox="0 0 820 820"
          style={{ position: "absolute", right: -260, top: -300 }}
        >
          <circle
            cx="410"
            cy="410"
            r="404"
            fill="none"
            stroke="#b9bdc7"
            strokeWidth="3"
          />
        </svg>
        {/* the brush field, bleeding from the left edge */}
        <svg
          width="560"
          height="630"
          viewBox="0 0 560 630"
          style={{ position: "absolute", left: -140, top: 0 }}
        >
          <path
            d="M-60 90 C 60 40, 230 70, 290 160 C 350 250, 300 330, 370 400 C 436 462, 410 560, 310 600 C 200 644, 40 630, -60 580 Z"
            fill="#7d3bc7"
            opacity="0.13"
          />
          <path
            d="M-70 180 C 20 130, 170 150, 220 230 C 268 302, 220 370, 280 430 C 330 482, 292 548, 205 570 C 108 594, -20 576, -70 530 Z"
            fill="#7d3bc7"
            opacity="0.2"
          />
          <path
            d="M-80 280 C -10 236, 100 248, 140 306 C 178 360, 144 412, 186 456 C 222 492, 188 534, 118 544 C 40 556, -50 534, -80 500 Z"
            fill="#7d3bc7"
            opacity="0.26"
          />
        </svg>

        <div
          style={{
            display: "flex",
            fontFamily: "IBM Plex Mono",
            fontSize: 24,
            color: "#5c616e",
            letterSpacing: 1,
          }}
        >
          {SITE_CONFIG.workingName} · built in the open
        </div>
        <div
          style={{
            display: "flex",
            fontFamily: "Space Grotesk",
            marginTop: 28,
            fontSize: 104,
            fontWeight: 600,
            color: "#22242e",
            letterSpacing: -4,
            lineHeight: 1,
            maxWidth: 900,
          }}
        >
          The Open Source Gateway
        </div>
        <div
          style={{
            display: "flex",
            fontFamily: "Space Grotesk",
            marginTop: 36,
            fontSize: 30,
            color: "#5c616e",
            maxWidth: 860,
            lineHeight: 1.35,
          }}
        >
          A gateway platform teams build, own, and answer for, measured by
          the Gateway Baseline.
        </div>
        <div
          style={{
            display: "flex",
            marginTop: 44,
            alignItems: "center",
            gap: 14,
          }}
        >
          <div
            style={{
              display: "flex",
              width: 46,
              height: 10,
              backgroundColor: "#e2882f",
            }}
          />
          <div
            style={{
              display: "flex",
              fontFamily: "IBM Plex Mono",
              fontSize: 24,
              color: "#22242e",
            }}
          >
            opensourcegateway.com
          </div>
        </div>
      </div>
    ),
    {
      ...size,
      fonts: [
        {
          name: "Space Grotesk",
          data: grotesk,
          weight: 600,
          style: "normal",
        },
        {
          name: "IBM Plex Mono",
          data: plexMono,
          weight: 400,
          style: "normal",
        },
      ],
    },
  );
}
