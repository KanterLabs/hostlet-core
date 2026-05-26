"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Box, ExternalLink, ListFilter, Plus, ScrollText } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, EmptyState, FilterTabs, KeyValueGrid, KeyValueItem, PageHeader, Panel, StatusPill } from "@/components/ui";

type Deployment = {
  id: string;
  status?: string | null;
  commitSha?: string | null;
  failure?: string | null;
  startedAt?: string | null;
  finishedAt?: string | null;
};

type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  containerPort?: number | null;
  healthPath?: string | null;
  memoryLimitMb?: number | null;
  cpuLimit?: number | null;
  rootDirectory: string;
  currentDeploymentId?: string | null;
  server?: { id: string; name: string; kind: string; status: string; publicIp?: string | null; lastSeenAt?: string | null } | null;
  latestDeployment?: Deployment | null;
  currentDeployment?: { status: string; publishedPort?: number | null; finishedAt?: string | null } | null;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  latestWebhook?: {
    status: string;
    ignoredReason?: string | null;
    commitSha?: string | null;
    branch?: string | null;
    createdAt?: string | null;
  } | null;
  health?: RuntimeHealth | null;
};

type RuntimeHealth = {
  status: string;
  httpStatus?: number | null;
  latencyMs?: number | null;
  failureCount?: number | null;
  lastError?: string | null;
  lastCheckedAt?: string | null;
  lastHealthyAt?: string | null;
};
type SessionPayload = {
  mode: "self_hosted" | "cloud";
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
  } | null;
};

export default function Apps() {
  const [apps, setApps] = useState<App[]>([]);
  const [session, setSession] = useState<SessionPayload | null>(null);
  const [message, setMessage] = useState("Loading apps...");
  const [filter, setFilter] = useState<"all" | "active" | "failed" | "public" | "healthy" | "degraded" | "unhealthy" | "unknown">("all");

  useEffect(() => {
    let active = true;
    async function loadApps() {
      try {
        const rows = await api<App[]>("/api/apps");
        if (!active) return;
        setApps(rows);
        setMessage(rows.length ? "" : "No apps yet.");
      } catch (e) {
        if (!active) return;
        setMessage(`Could not load apps. ${e instanceof Error ? e.message : "Sign in again."}`);
      }
    }
    loadApps();
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") loadApps();
    }, 10000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, []);
  useEffect(() => {
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
  }, []);

  const cloud = session?.mode === "cloud";
  const createDisabledReason = cloudCreateDisabledReason(session);
  const filtered = useMemo(() => {
    return apps.filter((app) => {
      if (filter === "active") return isActiveDeploy(app.latestDeployment?.status);
      if (filter === "failed") return app.latestDeployment?.status === "failed";
      if (filter === "public") return !!app.publicExposure;
      if (filter === "healthy") return app.health?.status === "healthy";
      if (filter === "degraded") return app.health?.status === "degraded";
      if (filter === "unhealthy") return app.health?.status === "unhealthy";
      if (filter === "unknown") return !app.health || app.health.status === "unknown";
      return true;
    });
  }, [apps, filter]);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Applications"
            title="Apps"
            description={cloud ? "Deployable projects, Hostlet Cloud URLs, latest health, and runtime state." : "Deployable projects, routes, latest health, automation, and public exposure."}
            actions={
              createDisabledReason ? (
                <button className="button" disabled title={createDisabledReason}><Plus size={16} />Create app</button>
              ) : (
                <Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>
              )
            }
          />

          <FilterTabs label="Filter" icon={ListFilter} value={filter} items={["all", "active", "failed", "public", "healthy", "degraded", "unhealthy", "unknown"] as const} onChange={setFilter} />

          {filtered.length > 0 ? (
            <div className="grid gap-4">
              {filtered.map((app) => (
                <Panel key={app.id} className="overflow-hidden" padded={false}>
                  <div className="flex flex-wrap items-start justify-between gap-4 p-4">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <Link href={`/apps/${app.id}`} className="truncate text-lg font-semibold hover:text-action">{app.name}</Link>
                        <StatusPill status={app.latestDeployment?.status || app.currentDeployment?.status || "not deployed"} />
                        <StatusPill status={app.health?.status || "unknown"} label={`health ${app.health?.status || "unknown"}`} />
                        <StatusPill status={app.server?.status || "offline"} label={`${cloud ? "worker" : "machine"} ${app.server?.status || "offline"}`} />
                      </div>
                      <p className="muted mt-1 break-all">{app.repoFullName} · {app.branch}</p>
                    </div>
                    <div className="flex shrink-0 flex-wrap gap-2">
                      {appVisitHref(app, cloud) && (
                        <a className="button" href={appVisitHref(app, cloud) || "#"} target="_blank" rel="noreferrer"><ExternalLink size={16} />Visit</a>
                      )}
                      <Link className="button-secondary" href={`/apps/${app.id}`}><Box size={16} />Open</Link>
                      {app.latestDeployment?.id && (
                        <Link className="button-secondary" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />Logs</Link>
                      )}
                    </div>
                  </div>

                  <KeyValueGrid>
                    <KeyValueItem label={cloud ? "Hostlet Cloud URL" : app.publicExposure ? "Public URL" : "Private URL"} value={appVisitLabel(app, cloud)} href={appVisitHref(app, cloud)} externalIcon={<ExternalLink size={13} />} />
                    <KeyValueItem label={cloud ? "Worker" : "Machine"} value={cloud ? "Hostlet Cloud managed worker" : `${app.server?.name || "Unknown"} · ${app.server?.kind || "remote"}`} />
                    <KeyValueItem label="Runtime" value={`:${app.containerPort || 3000}${app.healthPath || "/"}`} />
                    <KeyValueItem label="Latest deploy" value={deploymentSummary(app.latestDeployment)} />
                    <KeyValueItem label="Runtime health" value={healthSummary(app.health)} />
                    <KeyValueItem label="Commit" value={shortSha(app.latestDeployment?.commitSha)} />
                    <KeyValueItem label="Limits" value={cloud ? `${app.memoryLimitMb || 512} MB · ${app.cpuLimit || 0.5} CPU` : `${app.memoryLimitMb ? `${app.memoryLimitMb} MB` : "no memory cap"} · ${app.cpuLimit ? `${app.cpuLimit} CPU` : "no CPU cap"}`} />
                    {!cloud && <KeyValueItem label="Auto redeploy" value={app.autoDeploy ? "enabled" : "disabled"} />}
                    {!cloud && <KeyValueItem label="Webhook" value={webhookSummary(app.latestWebhook)} />}
                  </KeyValueGrid>

                  {app.latestDeployment?.failure && (
                    <div className="border-t border-red-100 bg-red-50 px-4 py-3 text-sm text-red-900">
                      {app.latestDeployment.failure}
                    </div>
                  )}
                </Panel>
              ))}
            </div>
          ) : (
            <EmptyState
              icon={Box}
              title={message || "No apps match this filter"}
              description={cloud ? "Create a cloud app from a GitHub repository, then deploy it to a Hostlet Cloud URL." : "Create an app from a GitHub repository, then deploy it to this Hostlet machine."}
              actionHref={createDisabledReason ? undefined : "/apps/new"}
              actionLabel={createDisabledReason ? undefined : "Create app"}
            />
          )}
    </AppShell>
  );
}

function shortSha(sha?: string | null) {
  if (!sha || sha === "HEAD") return sha || "No deploy yet";
  return sha.slice(0, 7);
}

function deploymentSummary(deployment?: Deployment | null) {
  if (!deployment) return "No deployments";
  const when = deployment.finishedAt || deployment.startedAt;
  return when ? `${deployment.status || "unknown"} · ${new Date(when).toLocaleString()}` : deployment.status || "unknown";
}

function webhookSummary(webhook?: App["latestWebhook"]) {
  if (!webhook) return "No branch push seen";
  const sha = webhook.commitSha ? ` ${webhook.commitSha.slice(0, 7)}` : "";
  return webhook.ignoredReason ? `${webhook.status}${sha}: ${webhook.ignoredReason}` : `${webhook.status}${sha}`;
}

function healthSummary(health?: RuntimeHealth | null) {
  if (!health) return "unknown";
  const bits = [health.status];
  if (health.httpStatus) bits.push(`HTTP ${health.httpStatus}`);
  if (typeof health.latencyMs === "number") bits.push(`${health.latencyMs} ms`);
  if (health.lastCheckedAt) bits.push(`checked ${new Date(health.lastCheckedAt).toLocaleTimeString()}`);
  return bits.join(" · ");
}

function appVisitHref(app: App, cloud = false) {
  if (!app.currentDeploymentId) return null;
  if (cloud) {
    const display = displayDomain(app.domain);
    if (!display) return null;
    return display.startsWith("http://") || display.startsWith("https://") ? display : `https://${display}`;
  }
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

function appVisitLabel(app: App, cloud = false) {
  if (cloud) return displayDomain(app.domain) || "No Hostlet Cloud URL";
  if (app.publicExposure) return displayDomain(app.domain) || "No public URL";
  const port = app.currentDeployment?.publishedPort;
  const host = privateAppHost(app);
  return port && host ? `${host}:${port}` : "Deploy to assign a private port";
}

function privateAppHost(app: App) {
  const host = app.server?.publicIp?.trim();
  if (host && host !== "127.0.0.1" && host !== "localhost" && host !== "0.0.0.0") return host;
  if (typeof window !== "undefined") return window.location.hostname;
  return host || null;
}

function displayDomain(domain: string) {
  if (!domain) return null;
  if (typeof window === "undefined") return domain;
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

function isActiveDeploy(status?: string | null) {
  return !!status && ["queued", "running", "building", "starting", "health_checking", "routing"].includes(status);
}

function cloudCreateDisabledReason(session: SessionPayload | null) {
  if (session?.mode !== "cloud") return "";
  if (!session.cloud?.githubInstalled) return "Install the Hostlet GitHub App before creating cloud apps.";
  if (!session.cloud.billingActive) return "Start a Stripe sandbox subscription before creating cloud apps.";
  if (session.cloud.nextStep !== "ready") return "Finish Hostlet Cloud setup before creating apps.";
  return "";
}
