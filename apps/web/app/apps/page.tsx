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
  server?: { id: string; name: string; kind: string; status: string; lastSeenAt?: string | null } | null;
  latestDeployment?: Deployment | null;
  currentDeployment?: { status: string; finishedAt?: string | null } | null;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  latestWebhook?: {
    status: string;
    ignoredReason?: string | null;
    commitSha?: string | null;
    branch?: string | null;
    createdAt?: string | null;
  } | null;
};

export default function Apps() {
  const [apps, setApps] = useState<App[]>([]);
  const [message, setMessage] = useState("Loading apps...");
  const [filter, setFilter] = useState<"all" | "active" | "failed" | "public">("all");

  useEffect(() => {
    api<App[]>("/api/apps")
      .then((rows) => {
        setApps(rows);
        setMessage(rows.length ? "" : "No apps yet.");
      })
      .catch((e) => setMessage(`Could not load apps. ${e instanceof Error ? e.message : "Sign in again."}`));
  }, []);

  const filtered = useMemo(() => {
    return apps.filter((app) => {
      if (filter === "active") return isActiveDeploy(app.latestDeployment?.status);
      if (filter === "failed") return app.latestDeployment?.status === "failed";
      if (filter === "public") return !!app.publicExposure;
      return true;
    });
  }, [apps, filter]);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Applications"
            title="Apps"
            description="Deployable projects, routes, latest health, automation, and public exposure."
            actions={<Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>}
          />

          <FilterTabs label="Filter" icon={ListFilter} value={filter} items={["all", "active", "failed", "public"] as const} onChange={setFilter} />

          {filtered.length > 0 ? (
            <div className="grid gap-4">
              {filtered.map((app) => (
                <Panel key={app.id} className="overflow-hidden" padded={false}>
                  <div className="flex flex-wrap items-start justify-between gap-4 p-4">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <Link href={`/apps/${app.id}`} className="truncate text-lg font-semibold hover:text-action">{app.name}</Link>
                        <StatusPill status={app.latestDeployment?.status || app.currentDeployment?.status || "not deployed"} />
                        <StatusPill status={app.server?.status || "offline"} label={`machine ${app.server?.status || "offline"}`} />
                      </div>
                      <p className="muted mt-1 break-all">{app.repoFullName} · {app.branch}</p>
                    </div>
                    <div className="flex shrink-0 flex-wrap gap-2">
                      <Link className="button-secondary" href={`/apps/${app.id}`}><Box size={16} />Open</Link>
                      {app.latestDeployment?.id && (
                        <Link className="button-secondary" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />Logs</Link>
                      )}
                    </div>
                  </div>

                  <KeyValueGrid>
                    <KeyValueItem label="Domain" value={displayDomain(app.domain)} href={domainHref(app)} externalIcon={<ExternalLink size={13} />} />
                    <KeyValueItem label="Machine" value={`${app.server?.name || "Unknown"} · ${app.server?.kind || "remote"}`} />
                    <KeyValueItem label="Runtime" value={`:${app.containerPort || 3000}${app.healthPath || "/"}`} />
                    <KeyValueItem label="Latest deploy" value={deploymentSummary(app.latestDeployment)} />
                    <KeyValueItem label="Commit" value={shortSha(app.latestDeployment?.commitSha)} />
                    <KeyValueItem label="Limits" value={`${app.memoryLimitMb ? `${app.memoryLimitMb} MB` : "no memory cap"} · ${app.cpuLimit ? `${app.cpuLimit} CPU` : "no CPU cap"}`} />
                    <KeyValueItem label="Auto redeploy" value={app.autoDeploy ? "enabled" : "disabled"} />
                    <KeyValueItem label="Webhook" value={webhookSummary(app.latestWebhook)} />
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
              description="Create an app from a GitHub repository, then deploy it to this Hostlet machine."
              actionHref="/apps/new"
              actionLabel="Create app"
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

function domainHref(app: App) {
  if (!app.publicExposure) return null;
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
