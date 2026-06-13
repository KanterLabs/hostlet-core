import { api } from "@/lib/api";
import { formatBytes } from "@/lib/format";
import type { AgentJob, App, ResourceStats } from "./appDetail.types";

// Re-export shim: the canonical implementations of these helpers now live in
// `@/lib/app-status` and `@/lib/app-links` so cloud web — which overrides this
// route file — can import them from `lib/`. Kept here so existing
// `./appDetailHelpers` import sites keep working.
export { webhookSummary, isActiveDeploy, shortSha, healthMetricDetail, healthEventSummary } from "@/lib/app-status";
export { displayDomain, appVisitHref, appVisitLabel, privateAppHost } from "@/lib/app-links";

export function rollbackDisabledReason(app: App | null, active: boolean) {
  if (!app) return "App details are still loading.";
  if (active) return "Wait for the active deployment to finish before rolling back.";
  if (app.runtimeKind === "compose") return "Compose rollback is disabled for Hostlet 0.5.0. Redeploy the target revision instead.";
  if (!app.currentDeploymentId) return "Deploy this app once before rolling back.";
  return "";
}

export function cpuDisplay(raw: string) {
  const value = Number.parseFloat(raw.replace("%", ""));
  if (Number.isFinite(value) && value <= 0) {
    return { value: "Idle", detail: `${raw} CPU` };
  }
  if (Number.isFinite(value) && value > 0 && value < 0.01) {
    return { value: "<0.01%", detail: `${raw} CPU` };
  }
  return { value: raw, detail: "Docker live sample" };
}

function metricBytes(bytes?: number | null) {
  return typeof bytes === "number" && Number.isFinite(bytes) && bytes >= 0 ? formatBytes(bytes) : null;
}

export function memoryDisplay(resources?: ResourceStats | null) {
  if (!resources) return { value: "waiting", detail: "no sample" };
  const used = metricBytes(resources.memoryUsageBytes);
  const limit = metricBytes(resources.memoryLimitBytes);
  if (used && limit) return { value: used, detail: `${limit} limit · ${resources.memoryPercent}` };
  if (used) return { value: used, detail: resources.memoryPercent };
  return { value: resources.memoryUsage, detail: resources.memoryPercent };
}

export function networkDisplay(resources?: ResourceStats | null) {
  if (!resources) return { value: "waiting", detail: "no sample" };
  const rx = metricBytes(resources.networkRxBytes);
  const tx = metricBytes(resources.networkTxBytes);
  if (rx && tx) return { value: `${rx} RX`, detail: `${tx} TX` };
  return { value: resources.networkIo, detail: "Docker live sample" };
}

export function diskDisplay(resources?: ResourceStats | null) {
  if (!resources) return { value: "waiting", detail: "no sample" };
  const read = metricBytes(resources.blockReadBytes);
  const written = metricBytes(resources.blockWriteBytes);
  if (read && written) return { value: `${read} read`, detail: `${written} written` };
  return { value: resources.blockIo, detail: "Docker live sample" };
}

export function pidsDisplay(resources?: ResourceStats | null) {
  if (!resources) return "waiting";
  return typeof resources.pidsCurrent === "number" ? String(resources.pidsCurrent) : resources.pids;
}

export async function waitForAgentJob(
  jobId: string,
  setMessage: (message: string) => void,
  // When false the loop exits immediately; defaults to always-active.
  isActive: () => boolean = () => true,
) {
  for (let attempt = 1; attempt <= 60; attempt += 1) {
    if (!isActive()) return;
    const job = await api<AgentJob>(`/api/agent-jobs/${jobId}`);
    if (job.status === "success") return;
    if (job.status === "failed") {
      throw new Error(job.failure || "Server cleanup failed.");
    }
    setMessage(`Server cleanup is ${job.status}. Waiting for confirmation...`);
    await new Promise((resolve) => setTimeout(resolve, 2000));
  }
  throw new Error("Server cleanup did not finish within 120 seconds.");
}
