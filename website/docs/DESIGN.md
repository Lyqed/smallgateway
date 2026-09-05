# Design brief — thegatewaybaseline.com & smallgateway.vercel.app

Current framing (5 September 2026): the project is smallgateway. Use that
name at smallgateway.vercel.app, the primary site address. Use the same
name in the header, headline, page title, and share image. The header carries
the only icon at the top of the page. Put experimental status in a separate
badge. Keep descriptions concrete and conversational.

The brief below records earlier design directions. Its older names and
ownership language have been superseded.

*The binding art direction for both sites. Derived from one photograph: the
Astra launch-site atrium — a rocket standing in a white circular gallery,
hard skylight geometry on brushed metal, and hand-painted murals bleeding
across the clean walls. Two sites, one design system, two temperaments.*

## 1. The direction, named

**Machined precision × hand-painted humanity.** Not "space aesthetic." The
photo's power is the collision: white curved architecture, perforated steel,
hard-edged sunlight — interrupted by murals a person painted by hand (an
astronaut holding a flower, a monarch butterfly, planets in saturated
splashes). The web translation:

- The *engineering layer* — layout, typography, data, the matrix — is
  precise, gridded, light, unornamented. It earns trust the way the spec
  does: by being exact.
- The *human layer* — hand-drawn SVG strokes, painted color fields, script
  annotations — deliberately breaks that grid at chosen moments. It is why
  anyone cares: people build this.

Neither layer is decoration for the other. The rule: **every painted element
annotates something real** (a principle, a check, a milestone), and **every
precise element could survive without the paint**.

And beneath both, a third register, used sparingly: **Kubrick stillness.**
The photo is one frame from *2001* — a machine built to leave Earth standing
silent in a white circular room, dwarfed by its own architecture. At a few
chosen moments each site goes still: perfect one-point symmetry, vast
negative space, and one deadpan sentence from the spec, alone. No paint, no
reassurance, no CTA. The existential note is quiet and earned — this
infrastructure watches, counts, and remembers, and somebody is responsible
for it. It must never tip into sci-fi theater; it is the silence between the
two louder layers.

This is light-first by conviction — the atrium is daylight. `color-scheme:
light`, no dark mode in v1. The single dark surface is the polished-floor
footer.

## 2. Tokens (copy verbatim into both sites)

```css
:root {
  color-scheme: light;

  /* Architecture */
  --surface-atrium: oklch(97.5% 0.004 95);   /* warm gallery white — page ground */
  --surface-panel: oklch(94.5% 0.005 250);   /* cool panel white — cards, wells */
  --steel: oklch(76% 0.01 250);              /* brushed rail — borders, rules */
  --steel-dark: oklch(52% 0.014 255);        /* fixture gray — secondary text */
  --floor: oklch(21% 0.012 270);             /* polished dark floor — footer, dark band */
  --ink: oklch(19% 0.016 270);               /* primary text */

  /* Murals (semantic, never decorative-only) */
  --monarch: oklch(69% 0.185 55);            /* monarch orange — leadership, emphasis, CTAs */
  --violet: oklch(54% 0.21 300);             /* mural violet — the human layer: annotations, community */
  --teal: oklch(64% 0.115 195);              /* sky teal — conformance, verified, success */
  --gold: oklch(80% 0.15 90);                /* sun gold — partial, in-progress, caution */
  --skylight: oklch(63% 0.14 250);           /* skylight blue — links, interactive */
  --blossom: oklch(58% 0.2 25);              /* blossom red — missing, errors, hard stops */

  /* Type scale */
  --text-base: clamp(1rem, 0.94rem + 0.3vw, 1.125rem);
  --text-hero: clamp(2.75rem, 1.2rem + 6.5vw, 7rem);
  --text-section: clamp(1.75rem, 1.2rem + 2.2vw, 3rem);

  /* Rhythm */
  --space-section: clamp(5rem, 3.5rem + 6vw, 11rem);

  /* Motion */
  --duration-fast: 150ms;
  --duration-normal: 320ms;
  --ease-out-expo: cubic-bezier(0.16, 1, 0.3, 1);
}
```

Semantic mapping is binding: teal = conforms/verified, gold = partial/in
progress, blossom = missing/error, monarch = leader/emphasis/primary action,
violet = human voice (annotations, community, contribution), skylight =
links. Never swap these roles.

## 3. Typography

Three families, each a voice — the third is the exception the concept pays
for (subset, loaded only where used):

| Family | Voice | Use |
|---|---|---|
| **Space Grotesk** (`next/font/google`) | the machined voice | display, headings, UI, body |
| **IBM Plex Mono** | the instrument voice | GB codes, data cells, dates, spec clauses, commit refs |
| **Shantell Sans** | the mural voice | hand annotations only — 4-8 instances per page, never body text, never navigation |

Display headlines set tight (`letter-spacing: -0.03em`, `line-height: 0.95`),
often uppercase for single words, mixed-case for sentences. Mono always at
smaller optical sizes with `letter-spacing: 0.02em`. Shantell in violet (or
monarch on dark), rotated −2° to +3°, positioned like margin notes.

## 4. The visual vocabulary

Recurring, reusable, restrained. Build each once as a component/SVG, reuse.

**Architecture (the grid's own ornament):**
- *Skylight bands* — hard-edged diagonal light: low-opacity linear-gradient
  stripes clipped at 18-24°, laid across white sections. Light, not lines.
- *Perforated rail* — SVG dot-grid texture band (the mesh railing), used as
  section dividers or card texture at ≤6% opacity.
- *The ring* — large circular outline elements (the circular gallery): a
  ring cropped by the viewport behind hero content, ring-segment dividers.
- *Polished floor* — the footer: `--floor` background with a faint vertical
  reflection gradient of the content above it.

**Murals (the hand's vocabulary — organic, saturated, grid-breaking):**
- *Brush fields* — irregular painted color blobs (SVG paths, 2-3 layered
  tones of one mural color) sitting **behind** or **bleeding across** a
  section edge; never perfectly rounded, never symmetric.
- *Orbit arcs* — thin hand-wobbled arcs with a small planet dot, crossing a
  section corner.
- *The monarch* — one butterfly SVG (hand-drawn path quality, monarch +
  floor-dark), used at most once per site, at the moment of transformation
  (the Baseline's "verified" moment; the Project's "contribute" moment).
- *Hand strokes* — circle-an-item ellipses, underline swashes, connector
  arrows; stroke-drawn (`stroke-dasharray` reveal on scroll), 2-3px, violet.
- *Spark marks* — tiny 4-point hand stars, sparse.

**Stillness (the Kubrick register — the third voice, rarest of all):**
- *Monolith bands* — full-bleed interstitial sections between major
  chapters: a single flat surface (`--floor` dark, or bare `--surface-atrium`),
  functionally empty, with one sentence of mono type perfectly centered on
  both axes. The sentence is always a real axiom from the spec or the
  principles, stated deadpan: "The gateway never just believes a tag." /
  "Someone is told when a cap is hit." / "If it breaks, you are
  responsible." Max two per site. Wide letter-spacing (0.08em+), small
  optical size against enormous space — the scale imbalance *is* the
  content.
- *The watching dot* — exactly once per site, inside one monolith band: a
  single small filled circle, perfectly centered above or below the
  sentence (`--monarch` on dark, `--blossom` on light). It is the alert
  that fires, the counter that runs — the system's unblinking eye. No
  animation beyond an optional slow (4s+) opacity breath, disabled under
  reduced motion. Never explained in copy.
- *Symmetry discipline* — monolith bands are strict one-point-perspective
  compositions (everything centered, generous identical margins), in
  deliberate contrast to the asymmetric editorial grid everywhere else.
  The snap into symmetry is what makes them uncanny; if the rest of the
  page were symmetric too, the effect dies.

Anti-kitsch rules: murals appear at **section boundaries and annotations**,
never as page-wide wallpaper; at most two mural moments per viewport; body
text never sits on paint; paint never carries information alone (a11y).
Stillness rules: monolith bands contain no paint, no links, no more than
one sentence; a monolith band and a mural moment never share a viewport —
stillness must be empty to work; never use HAL-red literally as a glowing
eye with lens-flare theatrics — a flat dot, nothing more.

## 5. Motion

Motion clarifies the three registers: the machined layer moves with
precision (fades, small translates, `--ease-out-expo`), the hand layer
*draws itself* (SVG stroke-dash reveals, 600-900ms), and the stillness
layer barely moves at all — monolith bands fade in slowly (1200ms, linear,
Kubrick pacing) and their sentence does not translate, only appears. IntersectionObserver-triggered,
compositor-friendly properties only, all gated behind
`prefers-reduced-motion` (reduced = everything visible, nothing animates).
Hover states: cards lift 2px with a hard-edged shadow shifting like
sunlight (offset shadow, not blur); links get a hand-underline swash on
hover where affordable, plain underline otherwise.

## 6. Shared skeleton

Both sites: Next.js 16.2.7 / React 19 / Tailwind v4 / TypeScript / pnpm —
the exact toolchain of antonbraverman.com (copy its config lineage:
`next.config.ts`, `tsconfig.json`, `postcss.config.mjs`, eslint). Standard
files: `layout.tsx` with skip link, semantic landmarks, `robots.ts`,
`sitemap.ts`, `icon.svg`, per-page metadata. Static prerender everything.
Performance budget: landing < 150kb JS gzipped — these are content sites;
ship close to zero client JS (IntersectionObserver reveals only).
Accessibility: WCAG AA contrast on all text (steel-dark on atrium passes;
verify every mural-color-on-white pairing ≥ 4.5:1 for text, or reserve for
non-text), keyboard-visible focus (2px monarch outline, offset 2px).

## 7. Site A — thegatewaybaseline.com

**Brand: "The Gateway Baseline." Temperament: the instrument.** 80%
machined / 20% mural. This is the neutral yardstick — it must read as a
spec, not a pitch. The paint here is *annotation*: the hand pointing at
what matters.

Single page, anchored nav (`Checks · Matrix · Method · History`), sections:

1. **Hero** — the ring motif behind; "The Gateway Baseline" in display
   caps; subline: nine checks a platform team can hold any LLM gateway to,
   verified against public documentation. One hand annotation: violet
   stroke circling the word "verified", Shantell note "*not vendor claims*".
   GB-1..GB-9 as a mono chip row.
2. **The nine checks** — the spec. One card per check: mono `GB-n` code,
   plain-language title, requirement text (MUST language), conformance
   note. Data ported **faithfully** from
   `/home/lyqed/website/src/lib/gateways.ts` (CRITERIA) — same words, no
   invention. Layout: editorial two-column rhythm, not a uniform card grid;
   GB-9 (the aspirational ninth) visually distinct (gold left rule +
   Shantell "*entered 14 July 2026 — awaits verification*").
3. **The matrix** — the heart. Port `GATEWAYS` cell data faithfully:
   8 gateways × 9 checks, sticky gateway column, mono cells; teal filled
   dot = conforms, gold half dot = partial, blossom open dot = missing,
   steel dash = not verified. Leader rows get a monarch "Leading" tint.
   Mobile: the existing dot-strip pattern. Every cell's note text available
   (details disclosure or title). Legend up front. The monarch butterfly
   sits at this section's edge — transformation happens here.
3b. **Monolith band** (after the matrix, before Method) — floor-dark,
   centered mono: "The gateway never just believes a tag." The watching
   dot sits above the sentence (monarch on dark). This is the site's
   still point: the matrix shouts with data, then the room goes quiet.
4. **Method** — how cells get verified (public docs only, dated snapshots,
   corrections via email), short and exact. Include the correction contact.
5. **Spec history** — `SPEC_HISTORY` dated entries as a mono timeline with
   ring-segment markers.
5b. **Monolith band** (before the reference-implementation section) — bare
   atrium white this time, centered mono: "Someone is told when a cap is
   hit." No dot (it was spent in 3b), no other content.
6. **Reference implementation** — one restrained cross-link band: "A
   community gateway is being built against this bar" → smallgateway.vercel.app
   (+ github.com/Lyqed/smallgateway).
7. **Footer** — polished floor: dark band, mono small print, sister-site
   link, "Verified cells, not marketing pages" as the standing line.

`site-config`: name "The Gateway Baseline", url https://thegatewaybaseline.com,
contact antonbraverman1@gmail.com (the tracker's correction address).

## 8. Site B — smallgateway.vercel.app

**Brand headline: "Open Source Gateway" (the project's working name,
"Open Source Gateway", appears in copy; final naming is an open decision —
docs/04 names the collision risk). Temperament: the mural.** 60% machined /
40% mural — this is the community site; the human layer leads. Same tokens,
same vocabulary, more paint: brush fields larger, annotations more frequent
(within the anti-kitsch caps).

Single page, anchored nav (`Principles · Architecture · Build · Contribute`):

1. **Hero** — brush field bleeding from the left edge behind the ring; "The
   Open Source Gateway" display; subline: a gateway platform teams build,
   own, and answer for — measured by the Gateway Baseline, in the open.
   Two CTAs: "Read the design docs" (monarch, → GitHub repo), "See the
   Baseline" (outline, → thegatewaybaseline.com).
2. **Principles** — three walls, each a panel with one hand annotation:
   *Two binaries plus Git* (the whole system, drawn as three hand-sketched
   boxes); *Defer, defer, defer* (Shantell: "*you don't need to buy
   anything — yet*"); *The six-month rule* (you own your change for its
   lifetime). Sourced from docs/00-principles.md — same claims, tightened
   copy.
3. **Architecture** — the canonical event stream as the centerpiece visual:
   the six events (`MessageStart → ContentDelta → ToolCallDelta →
   UsageDelta → MessageEnd / Error`) as stations on one hand-drawn line
   crossing a machined section; mono labels, violet strokes. Below, two
   compact panels: the data plane (streaming first-class, mid-stream
   enforcement) and the control plane (ArgoCD-for-gateway-fleets, Git as
   truth). Sourced from docs/02-architecture.md.
4. **Build status** — honest, dated: Phase 0 in progress; Spike A (event
   model over three wire formats — tests green) with what remains; Spike B
   (Pingora vs agentgateway) status; then the phase roadmap as a vertical
   mono timeline (from docs/04-build-plan.md). Gold = in progress, teal =
   done, steel = ahead. No invented progress.
4b. **Monolith band** (between Build status and the ownership contract) —
   floor-dark, centered mono: "It runs while you sleep." The watching dot
   below the sentence. The one still moment on a site that otherwise leads
   with the human layer — which is exactly why it lands.
5. **The ownership contract** — the mural moment (butterfly here): "If
   something breaks because of a change you made six months ago — you are
   responsible." Set large, half machined type / the key phrase circled by
   hand. Short paragraph on what that buys a community. (This section stays
   a mural moment, not a monolith — the contract is human warmth, not
   dread; the dread came one scroll earlier.)
6. **Contribute** — where to start: read the docs (repo link), the spikes,
   adapters as first contributions; violet panel.
7. **Footer** — polished floor, sister link to the Baseline, repo link.

`site-config`: name "Open Source Gateway", url
https://smallgateway.vercel.app.

## 9. What both sites must NOT be

The design-quality bar, stated as bans: no dark-mode dev-tool template; no
centered-hero-gradient-blob; no uniform card grids with identical radius and
shadow; no stock feature-triplet icons; no glassmorphism; no neon-on-black
"AI" styling; no parallax theater. If a section could be screenshotted into
any SaaS landing page, redo it. The photo is the test: would this moment
belong in that atrium?

## 10. Acceptance checklist (per site)

- [ ] `pnpm build` green, all pages static
- [ ] Tokens copied verbatim; semantic color roles respected
- [ ] Three fonts wired via next/font, Shantell only in annotations
- [ ] ≥ 4 of the vocabulary elements present, within anti-kitsch caps
- [ ] Stillness register present: 1-2 monolith bands, exactly one watching dot, symmetry discipline observed, stillness rules unbroken
- [ ] Baseline data ported faithfully (site A) / docs claims ported faithfully (site B) — zero invented facts
- [ ] Cross-links both directions between the two sites
- [ ] Keyboard nav + visible focus + reduced-motion verified
- [ ] Text contrast AA; no text on paint
- [ ] Reads as one system with the sister site, at a different temperature
