/**
 * Frozen point-in-time copies of the Gateway Baseline matrix.
 *
 * The live matrix in gateways.ts always shows the current judgment; a
 * snapshot is appended here when the matrix has just been verified end
 * to end, so future movement ("+4 since July") can be computed against
 * a dated, trusted state. Statuses only; reasoning notes live with the
 * live matrix. Nothing renders these yet; they exist so the timeline
 * feature has honest history to draw from on day one.
 */

import type { SupportStatus } from "@/lib/gateways";

export type MatrixSnapshot = {
  /** ISO date the snapshot was frozen. */
  date: string;
  /** One line on why this snapshot is trustworthy. */
  note: string;
  /** gatewayId -> criterionId -> status at the snapshot date. */
  cells: Record<string, Record<string, SupportStatus>>;
};

export const SNAPSHOTS: readonly MatrixSnapshot[] = [
  {
    date: "2026-08-03",
    note: "Second full verification: all 81 cells (9 gateways x 9 checks) re-checked against current vendor docs. GB-9 resolved for every gateway (was unverified); our own reference row added, scored from the code against the same bar. Notable movement: LiteLLM aws-invoice no->partial (PR #32797 merged), Cloudflare default-limit yes->partial and jwt-values no->partial, several GB-9 cells filled.",
    cells: {
      "the-gateway-baseline": {
        "enforced-keys": "yes",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "yes",
        "default-limit": "yes",
        alerts: "yes",
        "aws-invoice": "yes",
        "vertex-invoice": "yes",
        "live-changes": "yes",
      },
      agentgateway: {
        "enforced-keys": "yes",
        "jwt-values": "yes",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "partial",
        alerts: "no",
        "aws-invoice": "yes",
        "vertex-invoice": "no",
        "live-changes": "yes",
      },
      litellm: {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "yes",
        alerts: "yes",
        "aws-invoice": "partial",
        "vertex-invoice": "partial",
        "live-changes": "partial",
      },
      portkey: {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "no",
        "default-limit": "partial",
        alerts: "partial",
        "aws-invoice": "no",
        "vertex-invoice": "yes",
        "live-changes": "yes",
      },
      kong: {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "partial",
        alerts: "partial",
        "aws-invoice": "partial",
        "vertex-invoice": "no",
        "live-changes": "yes",
      },
      "envoy-ai": {
        "enforced-keys": "partial",
        "jwt-values": "yes",
        "static-values": "yes",
        "error-bodies": "yes",
        "default-limit": "partial",
        alerts: "no",
        "aws-invoice": "no",
        "vertex-invoice": "no",
        "live-changes": "yes",
      },
      "cloudflare-ai": {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "partial",
        "error-bodies": "no",
        "default-limit": "partial",
        alerts: "no",
        "aws-invoice": "no",
        "vertex-invoice": "no",
        "live-changes": "yes",
      },
      bifrost: {
        "enforced-keys": "yes",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "partial",
        alerts: "yes",
        "aws-invoice": "partial",
        "vertex-invoice": "no",
        "live-changes": "partial",
      },
      helicone: {
        "enforced-keys": "no",
        "jwt-values": "no",
        "static-values": "no",
        "error-bodies": "no",
        "default-limit": "partial",
        alerts: "yes",
        "aws-invoice": "no",
        "vertex-invoice": "no",
        "live-changes": "no",
      },
    },
  },
  {
    date: "2026-07-13",
    note: "First verified snapshot: all 64 cells checked against public documentation, the same day the checks were sharpened and the two invoice checks (GB-7, GB-8) were added.",
    cells: {
      agentgateway: {
        "enforced-keys": "yes",
        "jwt-values": "yes",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "partial",
        alerts: "no",
        "aws-invoice": "yes",
        "vertex-invoice": "no",
      },
      litellm: {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "yes",
        alerts: "yes",
        "aws-invoice": "no",
        "vertex-invoice": "partial",
      },
      portkey: {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "no",
        "default-limit": "partial",
        alerts: "partial",
        "aws-invoice": "no",
        "vertex-invoice": "yes",
      },
      kong: {
        "enforced-keys": "partial",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "yes",
        "default-limit": "partial",
        alerts: "partial",
        "aws-invoice": "partial",
        "vertex-invoice": "no",
      },
      "envoy-ai": {
        "enforced-keys": "partial",
        "jwt-values": "yes",
        "static-values": "yes",
        "error-bodies": "yes",
        "default-limit": "partial",
        alerts: "no",
        "aws-invoice": "no",
        "vertex-invoice": "no",
      },
      "cloudflare-ai": {
        "enforced-keys": "partial",
        "jwt-values": "no",
        "static-values": "partial",
        "error-bodies": "no",
        "default-limit": "yes",
        alerts: "no",
        "aws-invoice": "no",
        "vertex-invoice": "no",
      },
      bifrost: {
        "enforced-keys": "yes",
        "jwt-values": "partial",
        "static-values": "yes",
        "error-bodies": "partial",
        "default-limit": "partial",
        alerts: "partial",
        "aws-invoice": "partial",
        "vertex-invoice": "no",
      },
      helicone: {
        "enforced-keys": "no",
        "jwt-values": "no",
        "static-values": "partial",
        "error-bodies": "no",
        "default-limit": "partial",
        alerts: "yes",
        "aws-invoice": "no",
        "vertex-invoice": "no",
      },
    },
  },
] as const;
