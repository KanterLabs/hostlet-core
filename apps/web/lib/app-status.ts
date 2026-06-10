// Deploy / webhook / health status helpers shared by the apps list and the app
// detail page. These are pure presentation helpers (no `window` access) and are
// safe to import from both client and server components.

import { formatTimestamp } from "@/lib/time";
import type { App, RuntimeHealth, RuntimeHealthEvent } from "@/app/apps/[id]/appDetail.types";

export function webhookSummary(webhook?: App["latestWebhook"] | null) {
  if (!webhook) return "No push seen";
  const sha = webhook.commitSha ? ` ${webhook.commitSha.slice(0, 7)}` : "";
  return webhook.ignoredReason ? `ignored${sha}: ${webhook.ignoredReason}` : `${webhook.status}${sha}`;
}

export function isActiveDeploy(status?: string | null) {
  return !!status && ["queued", "running", "building", "starting", "health_checking", "routing"].includes(status);
}

export function shortSha(sha?: string | null) {
  if (!sha || sha === "HEAD") return sha || "No deploy yet";
  return sha.slice(0, 7);
}

export function healthMetricDetail(health?: RuntimeHealth | null) {
  if (!health) return "waiting for agent";
  if (health.lastError) return health.lastError;
  if (typeof health.latencyMs === "number") return `${health.latencyMs} ms`;
  return health.lastCheckedAt ? `checked ${formatTimestamp(health.lastCheckedAt, "time")}` : "not checked yet";
}

export function healthEventSummary(event: RuntimeHealthEvent) {
  const bits = [];
  if (event.httpStatus) bits.push(`HTTP ${event.httpStatus}`);
  if (typeof event.latencyMs === "number") bits.push(`${event.latencyMs} ms`);
  return bits.length ? bits.join(" · ") : "no response data";
}
