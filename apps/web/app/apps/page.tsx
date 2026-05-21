"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Box, ExternalLink, ListFilter, Plus, ScrollText } from "lucide-react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";
import { EmptyState, PageHeader, StatusPill } from "@/components/ui";

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
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
          <PageHeader
            eyebrow="Applications"
            title="Apps"
            description="Deployable projects, routes, latest health, automation, and public exposure."
            actions={<Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>}
          />

          <div className="mb-5 flex flex-wrap items-center gap-3 rounded-lg border border-line bg-surface p-2 shadow-sm shadow-neutral-950/5">
            <div className="flex items-center gap-2 px-2 text-sm font-medium text-muted">
              <ListFilter size={16} />
              Filter
            </div>
            <div className="flex flex-wrap gap-2">
              {(["all", "active", "failed", "public"] as const).map((item) => (
                <button key={item} className={`${filter === item ? "" : "button-secondary"} min-h-8 px-3 py-1.5 capitalize`} onClick={() => setFilter(item)}>
                  {item}
                </button>
              ))}
            </div>
          </div>

          {filtered.length > 0 ? (
            <div className="grid gap-4">
              {filtered.map((app) => (
                <article key={app.id} className="panel overflow-hidden">
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

                  <div className="grid border-t border-line md:grid-cols-4">
                    <Info label="Domain" value={displayDomain(app.domain)} href={domainHref(app)} />
                    <Info label="Machine" value={`${app.server?.name || "Unknown"} · ${app.server?.kind || "remote"}`} />
                    <Info label="Runtime" value={`:${app.containerPort || 3000}${app.healthPath || "/"}`} />
                    <Info label="Latest deploy" value={deploymentSummary(app.latestDeployment)} />
                    <Info label="Commit" value={shortSha(app.latestDeployment?.commitSha)} />
                    <Info label="Limits" value={`${app.memoryLimitMb ? `${app.memoryLimitMb} MB` : "no memory cap"} · ${app.cpuLimit ? `${app.cpuLimit} CPU` : "no CPU cap"}`} />
                    <Info label="Auto redeploy" value={app.autoDeploy ? "enabled" : "disabled"} />
                    <Info label="Webhook" value={webhookSummary(app.latestWebhook)} />
                  </div>

                  {app.latestDeployment?.failure && (
                    <div className="border-t border-red-100 bg-red-50 px-4 py-3 text-sm text-red-900">
                      {app.latestDeployment.failure}
                    </div>
                  )}
                </article>
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
        </div>
      </section>
    </main>
  );
}

function Info({ label, value, href }: { label: string; value?: string | null; href?: string | null }) {
  return (
    <div className="min-w-0 border-t border-line px-4 py-3 first:border-t-0 md:border-l md:border-t-0 md:first:border-l-0">
      <div className="eyebrow">{label}</div>
      {href ? (
        <a className="mt-1 flex items-center gap-1 truncate text-sm font-medium hover:text-action" href={href} target="_blank" rel="noreferrer">
          <span className="truncate">{value || "Not set"}</span>
          <ExternalLink size={13} />
        </a>
      ) : (
        <div className="mt-1 truncate text-sm font-medium">{value || "Not set"}</div>
      )}
    </div>
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
