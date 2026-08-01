# Mural direction — maximal / anarchist

The opensourcegateway site (community site only, not the baseline) leans hard
into the mural side of the design. This is the loud half of "machined precision
x hand-painted humanity": the engineering layer (spec, build status, the event
stream, the numbers) stays present and readable, but color and hand-art
dominate and the machined grid is violently interrupted, not merely annotated.

## Three reference energies, fused

1. **The Astra atrium mural (full color).** An astronaut reaching for a flower,
   a monarch butterfly on a deep-blue planet, color-splash arcs (gold, violet,
   teal) bleeding across white curved walls and brushed steel. Painterly,
   humanist, hopeful. This is the heart image.
2. **Graffiti collage (the rock-legends wall).** Spray-paint tags, drips,
   torn newsprint underneath, marker scrawl, a hand-drawn anarchy-style star,
   color words sprayed over a grid of clippings. Street, layered, unruly.
3. **Torn-paper reveal (the ripped cover).** Hard machined shapes violently
   torn to reveal saturated color and imagery underneath. The tear is the
   collision made literal: clean surface ripped open, human color bleeding out.

Plus: Toy Story / childish crayon, quick sketch lines, semi-anarchist marks.
Nothing too precious. Human hands, not a brand system.

## How it reads on the web

- **The machined layer is the wall.** Crisp mono type, the spec, the build
  status grid, the event-stream diagram, hairline steel rules. Kept legible.
- **The murals bleed across it.** Full-color painterly gradient fields, the
  astronaut and monarch as hand-drawn inline SVG figures, color arcs that
  ignore the grid and cross section boundaries. Torn-paper edges where a clean
  panel rips open to color underneath.
- **Graffiti marks punctuate.** Spray tags, drips, crayon scrawl, a hand star,
  sketchy underlines and circles, sprayed color words. Sparse enough to read as
  intentional, dense enough to feel unruly.
- **The collision is the point.** Every place the engineering is at its most
  precise (a number, a spec clause, the matrix), that is exactly where the hand
  breaks in. Precision and paint share the same square inch.

## Production

- **Hand-built inline SVG only.** No external image hosts (the build forbids
  them). Painterly gradient blobs, torn-paper clip-paths, hand-drawn figures
  (astronaut, monarch, planets), graffiti strokes and tags, crayon textures,
  all as self-contained, themeable SVG + CSS. Grain and spray-noise via CSS
  filters / SVG turbulence.
- **Reuse and escalate** the existing marks (BrushField, Monarch, EventStream)
  rather than starting from zero.

## Non-negotiables (so anarchy stays a design, not a mess)

- Body text never sits directly on busy paint: it gets a clean surface or a
  legible scrim. Contrast stays AA on all readable text.
- The build-status facts, the phase progress, the event-stream labels stay
  accurate and legible. No mural obscures a fact.
- Reduced-motion honored: paint does not animate for users who opt out; the
  page is fully readable with zero JS.
- Loud, not broken: no horizontal overflow, works at 360px to 1920px.
- The color roles still mean things (monarch = emphasis, teal = verified, etc.),
  even amid the chaos.
