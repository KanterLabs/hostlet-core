// Deploy / webhook / health status helpers shared by the apps list and the app
// detail page. These are pure presentation helpers (no `window` access) and are
// safe to import from both client and server components.
//
// Minimal structural types are defined here so this module is self-contained and
// works in both the core and cloud overlays (the cloud overlay replaces app/ with
// cloud-specific routes, so imports from @/app/apps/[id]/... would break here).

import { formatTimestamp } from "@/lib/time";

// Minimal webhook shape required by webhookSummary.
export type Webhook = {
  status: string;
  ignoredReason?: string | null;
  commitSha?: string | null;
};

// Minimal health shapes required by healthMetricDetail / healthEventSummary.
export type RuntimeHealthSummary = {
  lastError?: string | null;
  latencyMs?: number | null;
  lastCheckedAt?: string | null;
};

export type RuntimeHealthEventSummary = {
  httpStatus?: number | null;
  latencyMs?: number | null;
};

export type Deployment = {
  id: string;
  status?: string | null;
  commitSha?: string | null;
  failure?: string | null;
  startedAt?: string | null;
  finishedAt?: string | null;
};

export function webhookSummary(webhook?: Webhook | null) {
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

export function healthMetricDetail(health?: RuntimeHealthSummary | null) {
  if (!health) return "waiting for agent";
  if (health.lastError) return health.lastError;
  if (typeof health.latencyMs === "number") return `${health.latencyMs} ms`;
  return health.lastCheckedAt ? `checked ${formatTimestamp(health.lastCheckedAt, "time")}` : "not checked yet";
}

export function healthEventSummary(event: RuntimeHealthEventSummary) {
  const bits = [];
  if (event.httpStatus) bits.push(`HTTP ${event.httpStatus}`);
  if (typeof event.latencyMs === "number") bits.push(`${event.latencyMs} ms`);
  return bits.length ? bits.join(" · ") : "no response data";
}

export function deploymentSummary(deployment?: Deployment | null) {
  if (!deployment) return "No deployments";
  const when = deployment.finishedAt || deployment.startedAt;
  return when ? `${deployment.status || "unknown"} · ${formatTimestamp(when)}` : deployment.status || "unknown";
}
