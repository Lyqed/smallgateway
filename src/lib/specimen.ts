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
      "Runtime state lives in memory and Git holds the truth. Nothing your traffic depends on can be down because a database is down.",
    kind: "bounded",
  },
  {
    value: "~11μs",
    label: "cost of one sandboxed extension call",
    source:
      "Measured on the WASM path, then used as a budget: per-event streaming hooks stay switched off because that cost times every token is not worth paying.",
    kind: "measured",
  },
  {
    value: "17",
    label: "real transcripts behind the error bound",
    source:
      "Recorded from live traffic rather than written by hand, because fixtures you author agree with you and traffic does not.",
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
    name: "The data plane",
    role: "Sits in the request path and does the work",
    detail:
      "A proxy that reads every provider dialect into one internal event model, meters tokens as they stream, redacts on the way past, and stops a request when its budget is gone. It reads a file, it serves traffic. If the rest of this page disappeared it would keep doing that.",
    standalone: true,
  },
  {
    index: "02",
    name: "The control plane",
    role: "Sits beside the path and keeps the fleet honest",
    detail:
      "It compiles what is in Git into exactly what each data plane should be running, ships it, watches for divergence, and pulls anything that drifted back into line. It is the interesting half, and it is also the half you can decline.",
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
    title: "It arrives wearing a provider's clothes",
    body: "Server-sent events from one vendor, a different event framing from another, raw binary frames with their own checksums from a third. Three shapes for one idea.",
    holds: "No dialect gets a privileged path through the code",
  },
  {
    index: "02",
    title: "It becomes one internal shape",
    body: "Adapters translate each dialect into a single event model. Replay the same response at chunk sizes of one byte, seven, sixty-four, or all at once, and the events that come out are identical.",
    holds: "Chunk boundaries are an accident of the network, never of the meaning",
  },
  {
    index: "03",
    title: "It is counted while it moves",
    body: "Tokens are tallied as they pass, then reconciled against the provider's own final count. Nothing is held back to be counted at the end, because holding it back would be the same as breaking streaming.",
    holds: "The response is never buffered whole",
  },
  {
    index: "04",
    title: "It stops when the money runs out",
    body: "A budget exhausted halfway through a generation ends that stream deliberately, with a terminal event the operator chose, in the dialect the caller is already speaking.",
    holds: "A cut looks like an ending, never like a crash",
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
    claim: "Token estimates land within about half of the true count",
    method:
      "Seventeen recorded transcripts replayed against the provider's own reported usage.",
    caveat:
      "All but one stream. The misses are structural rather than random: tool-call scaffolding and very short responses are where the estimate is worst.",
  },
  {
    claim: "Spending stays under the cap across a whole fleet",
    method:
      "Each gateway holds a share of the budget, and the shares provably sum to no more than the cap.",
    caveat:
      "During a network partition, spend can exceed the cap by a bounded amount. The bound is published rather than hidden, because a system that claims a hard cap under partition is claiming something it cannot do.",
  },
  {
    claim: "A bad extension cannot take the gateway down",
    method:
      "Extensions run sandboxed with no ambient access, on a fuel budget, interruptible mid-execution. They must be signed.",
    caveat:
      "This is why per-event hooks are off. The safety costs about eleven microseconds each time, which is affordable once per request and indefensible once per token.",
  },
  {
    claim: "The fleet's running config can be rebuilt from a commit",
    method:
      "Git is the source of truth, and every rollout is a rendered snapshot tied to the commit that produced it.",
    caveat:
      "Break-glass exists for the night when Git is not the fastest way to fix production. It is visible, it expires on its own, and it is the exception that the design admits to.",
  },
] as const;

export type OpenItem = {
  name: string;
  body: string;
};

/** Not built, deliberately. Stated before anyone has to ask. */
export const NOT_YET: readonly OpenItem[] = [
  {
    name: "Kubernetes-native deployment",
    body: "Custom resources, an operator, a production chart. The control plane already talks to any data plane over its own protocol, so this is ergonomics rather than capability. It is next, and it is not claimed today.",
  },
  {
    name: "A public repository",
    body: "The code is private for now. That is a judgment call about timing, not a permanent condition, and it is the thing standing between this page and a reader who wants to check it.",
  },
  {
    name: "Durable spend counters",
    body: "Counters live in memory. Making them survive a restart means putting a database somewhere near the request path, and that trade has not earned itself yet.",
  },
  {
    name: "Identity from a verified login",
    body: "Written and working, switched off. It ships when the login story around it is worth turning on, and not on the day it merely compiles.",
  },
] as const;
