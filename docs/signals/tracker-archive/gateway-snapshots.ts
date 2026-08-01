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
