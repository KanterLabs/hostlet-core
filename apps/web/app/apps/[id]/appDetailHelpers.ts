import { api } from "@/lib/api";
import { formatTimestamp } from "@/lib/time";
import type { AgentJob, App, RuntimeHealth, RuntimeHealthEvent } from "./appDetail.types";

export function webhookSummary(webhook?: App["latestWebhook"] | null) {
  if (!webhook) return "No push seen";
  const sha = webhook.commitSha ? ` ${webhook.commitSha.slice(0, 7)}` : "";
  return webhook.ignoredReason ? `ignored${sha}: ${webhook.ignoredReason}` : `${webhook.status}${sha}`;
}

export function isActiveDeploy(status?: string | null) {
  return !!status && ["queued", "running", "building", "starting", "health_checking", "routing"].includes(status);
}

export function rollbackDisabledReason(app: App | null, active: boolean) {
  if (!app) return "App details are still loading.";
  if (active) return "Wait for the active deployment to finish before rolling back.";
  if (app.runtimeKind === "compose") return "Compose rollback is disabled for Hostlet 0.5.0. Redeploy the target revision instead.";
  if (!app.currentDeploymentId) return "Deploy this app once before rolling back.";
  return "";
}

export function shortSha(sha?: string | null) {
  if (!sha || sha === "HEAD") return sha || "No deploy yet";
  return sha.slice(0, 7);
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

export async function waitForAgentJob(jobId: string, setMessage: (message: string) => void) {
  for (let attempt = 1; attempt <= 60; attempt += 1) {
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

export function displayDomain(domain: string) {
  if (!domain || typeof window === "undefined") return domain;
  try {
    const withProtocol = domain.startsWith("http://") || domain.startsWith("https://") ? domain : `http://${domain}`;
    const url = new URL(withProtocol);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1") {
      url.hostname = window.location.hostname;
      return url.host + url.pathname.replace(/\/$/, "");
    }
  } catch {
    return domain;
  }
  return domain;
}

export function appVisitHref(app?: App | null) {
  if (!app?.currentDeploymentId) return null;
  if (!app.publicExposure) {
    const port = app.currentDeployment?.publishedPort;
    const host = privateAppHost(app);
    return port && host ? `http://${host}:${port}` : null;
  }
  const display = displayDomain(app.domain);
  if (!display) return null;
  if (display.startsWith("http://") || display.startsWith("https://")) return display;
  try {
    const url = new URL(`http://${display}`);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1" || /^[\d.]+$/.test(url.hostname)) {
      return `http://${display}`;
    }
  } catch {
    return null;
  }
  return `https://${display}`;
}

export function appVisitLabel(app: App) {
  if (app.publicExposure) return displayDomain(app.domain) || "No public URL";
  const port = app.currentDeployment?.publishedPort;
  const host = privateAppHost(app);
  return port && host ? `${host}:${port}` : "Deploy to assign a private port";
}

export function privateAppHost(app: App) {
  const host = app.server?.publicIp?.trim();
  if (host && host !== "127.0.0.1" && host !== "localhost" && host !== "0.0.0.0") return host;
  if (typeof window !== "undefined") return window.location.hostname;
  return host || null;
}
