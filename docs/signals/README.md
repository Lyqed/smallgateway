# Signals and the tracker archive

This directory preserves the gateway-comparison work so it survives regardless
of what either public site becomes. thegatewaybaseline.com was recentered on
1 August 2026 to state the cost-attribution thesis alone, dropping the
eight-gateway conformance matrix. Nothing here was lost in that move; this
directory is the durable home for the comparison apparatus if it is ever
wanted again.

## What is here

- `2026-08-01.md` — the upstream signals sweep for the window 2026-07-13 to
  2026-08-01. Per-gateway changes since the last verification snapshot: merged
  PRs, release versions, dead or moved documentation links, and the current
  state of every tracked reference (TRACKED_REFS). Every claim carries a URL
  and a date, verified against the GitHub API and the live pages. The report
  moves no cell; it is the worklist a re-verification pass would start from.

- `tracker-archive/` — a point-in-time copy of the tracker's source of record,
  taken from the personal-site repo on 1 August 2026:
  - `gateways.ts` — the CRITERIA (GB-1 through GB-9), the eight tracked
    gateways with every cell status and note, the doc URLs each cell cites,
    SPEC_HISTORY, and the scoring helpers.
  - `gateway-snapshots.ts` — the frozen matrix snapshots.
  These are a reference copy, not a live import. The authoritative originals,
  while they exist, are in the personal-site repo under `src/lib/`; this copy
  exists so the data outlives that repo.

## If the comparison tracker is ever revived

1. Start from `2026-08-01.md`: it names which cells likely moved and what to
   re-verify first (as of that date, the highest-priority item was
   agentgateway's native Vertex path shipping in v1.4.0, which changes the
   GB-8 cell for that gateway).
2. Re-verify each flagged cell against live documentation before changing any
   value. The report is signals only; it is not a verified diff.
3. The `tracker-archive/` data is the schema and the last-known state to build
   the new matrix from. Refresh it against current docs; do not ship it as-is.

## Provenance

The signals sweep and the archive both date to 1 August 2026. The tracker data
was authored earlier (see the personal-site repo history). Treat every value
as last-verified on its own date, not as current truth.
