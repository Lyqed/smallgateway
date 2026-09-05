/**
 * The specimen data: every figure the page states, in one file, so a
 * reader can audit the whole claim surface without reading the markup.
 *
 * Rule for this file: if a number cannot be pointed at something that
 * produced it, it does not belong here. "Measured" means a run wrote it
 * down. "Bounded" means the code refuses to exceed it. "Chosen" means a
 * person decided and the reasoning is written down somewhere public.
 */

export type Kind = "measured" | "bounded" | "chosen";

export type Figure = {
  /** The value, set large. Kept short enough to read at a glance. */
  value: string;
  /** What the value is of. Sentence case, no trailing period. */
  label: string;
  /** Where it came from. This is the part that makes the number honest. */
  source: string;
  kind: Kind;
};

/** The four figures that carry the whole argument. */
export const HEADLINE_FIGURES: readonly Figure[] = [
  {
    value: "2",
    label: "binaries in the whole system",
    source:
      "One data plane, one control plane. A team can run the data plane alone, from a file on disk, and never install the second.",
    kind: "chosen",
  },
  {
    value: "0",
    label: "databases in the request path",
    source:
      "The gateway keeps runtime state in memory and can read configuration from a local file. Its request path makes no database calls.",
    kind: "bounded",
  },
  {
    value: "~11μs",
    label: "cost of one sandboxed extension call",
    source:
      "Measured on the WASM path in a local benchmark. Per-event streaming hooks are off by default. This is an extension measurement, not overall request latency.",
    kind: "measured",
  },
  {
    value: "17",
    label: "real transcripts behind the error bound",
    source:
      "Recorded from live traffic and replayed to compare token estimates with provider-reported usage. A small sample, not a guarantee for every model.",
    kind: "measured",
  },
] as const;

export type ShapePiece = {
  index: string;
  name: string;
  role: string;
  detail: string;
  /** Runs standalone with no other component present. */
  standalone: boolean;
};

/** What the two binaries actually are. */
export const SHAPE: readonly ShapePiece[] = [
  {
    index: "01",
    name: "gatewayd",
    role: "The request proxy",
    detail:
      "Forwards requests to model providers, resolves caller attribution, records token usage, and applies token limits. Supported adapters read streaming responses as they arrive. Run it with a local configuration file.",
    standalone: true,
  },
  {
    index: "02",
    name: "gatewayctl",
    role: "Optional configuration management",
    detail:
      "Renders configuration from Git, distributes snapshots to gateway instances, and tracks which version each instance is running. This code is available when several gateways need shared configuration.",
    standalone: false,
  },
] as const;

export type PathStep = {
  index: string;
  title: string;
  body: string;
  /** The invariant this step holds. Rendered as the step's fine print. */
  holds: string;
};

/** What happens to one request, in order. */
export const PATH: readonly PathStep[] = [
  {
    index: "01",
    title: "Read the provider response",
    body: "Providers use different streaming formats, including server-sent events and binary event frames. The configured adapter parses the supported format.",
    holds: "Support is checked per provider and request shape",
  },
  {
    index: "02",
    title: "Translate into internal events",
    body: "Adapters expose content and usage through a shared event model. Replay tests check that splitting the same response into different network chunks produces the same events.",
    holds: "Chunk boundaries should not change the parsed events",
  },
  {
    index: "03",
    title: "Track usage while forwarding",
    body: "Token estimates support live enforcement. A provider's final usage count replaces the estimate when available. Missing usage and interrupted responses have documented limits.",
    holds: "Token usage is not a billed dollar amount",
  },
  {
    index: "04",
    title: "Apply the token limit",
    body: "An exhausted token allowance can end a stream with the operator's configured terminal event. The local estimate and any distributed allowance affect when that happens.",
    holds: "Limits are measured in tokens",
  },
] as const;

export type Measured = {
  claim: string;
  method: string;
  /** The uncomfortable part. Every entry has one. */
  caveat: string;
};

/**
 * Claims paired with how they were checked, and with what is still wrong
 * with them. The caveat column is the point of this section.
 */
export const MEASURED: readonly Measured[] = [
  {
    claim: "Token estimates were within about half of the reported count in most sample streams",
    method:
      "Seventeen recorded transcripts replayed against the provider's own reported usage.",
    caveat:
      "All but one stream. The misses are structural rather than random: tool-call scaffolding and very short responses are where the estimate is worst.",
  },
  {
    claim: "Gateway instances can share a token allowance",
    method:
      "The control plane allocates token shares to gateways. Tests exercise enforcement and measure overshoot when a gateway loses its connection.",
    caveat:
      "Token usage can exceed the configured allowance during a network partition. The documented bound depends on the configuration. These are token limits, not dollar budgets.",
  },
  {
    claim: "Extensions run with execution limits",
    method:
      "Extensions run sandboxed with no ambient access, on a fuel budget, interruptible mid-execution. They must be signed.",
    caveat:
      "A local benchmark measured about eleven microseconds per call. Per-event hooks are off by default. Sandboxing does not establish that every extension or workload is safe.",
  },
  {
    claim: "The fleet's running config can be rebuilt from a commit",
    method:
      "Git is the source of truth, and every rollout is a rendered snapshot tied to the commit that produced it.",
    caveat:
      "Temporary overrides can differ from Git until they expire. This describes configuration history, not a durable record of every request.",
  },
] as const;
