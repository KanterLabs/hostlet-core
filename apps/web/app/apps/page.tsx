"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";

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

  useEffect(() => {
    api<App[]>("/api/apps")
      .then((rows) => {
        setApps(rows);
        setMessage(rows.length ? "" : "Create an app to deploy your first repo.");
      })
      .catch((e) => setMessage(`Could not load apps. ${e instanceof Error ? e.message : "Sign in again."}`));
  }, []);

  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <div className="mb-6 flex items-center justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold">Apps</h1>
            <p className="muted mt-1">Deployable projects and their latest state.</p>
          </div>
          <Link className="button shrink-0" href="/apps/new">Create app</Link>
        </div>

        <div className="grid gap-3">
          {apps.map((app) => (
            <article key={app.id} className="rounded-lg border border-line bg-white p-4">
              <div className="flex flex-wrap items-start justify-between gap-4">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <Link href={`/apps/${app.id}`} className="text-lg font-semibold hover:text-action">{app.name}</Link>
                    <StatusBadge status={app.latestDeployment?.status || app.currentDeployment?.status || "not deployed"} />
                    <ServerBadge status={app.server?.status || "offline"} />
                  </div>
                  <p className="muted mt-1 break-all">{app.repoFullName} · {app.branch}</p>
                </div>
                <div className="flex gap-2">
                  <Link className="button bg-neutral-800 hover:bg-neutral-900" href={`/apps/${app.id}`}>Open</Link>
                  {app.latestDeployment?.id && (
                    <Link className="button bg-white text-neutral-900 ring-1 ring-line hover:bg-panel" href={`/deployments/${app.latestDeployment.id}`}>Logs</Link>
                  )}
                </div>
              </div>

              <div className="mt-4 grid gap-3 md:grid-cols-3">
                <Info label="Domain" value={displayDomain(app.domain)} href={domainHref(app)} />
                <Info label="Machine" value={`${app.server?.name || "Unknown"} · ${app.server?.kind || "remote"}`} />
                <Info label="Container" value={`:${app.containerPort || 3000}${app.healthPath || "/"}`} />
                <Info label="Latest deploy" value={deploymentSummary(app.latestDeployment)} />
                <Info label="Commit" value={shortSha(app.latestDeployment?.commitSha)} />
                <Info label="Limits" value={`${app.memoryLimitMb ? `${app.memoryLimitMb} MB` : "no memory cap"} · ${app.cpuLimit ? `${app.cpuLimit} CPU` : "no CPU cap"}`} />
                <Info label="Auto redeploy" value={app.autoDeploy ? "enabled" : "disabled"} />
                <Info label="Webhook" value={webhookSummary(app.latestWebhook)} />
              </div>

              {app.latestDeployment?.failure && (
                <div className="mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-900">
                  {app.latestDeployment.failure}
                </div>
              )}
            </article>
          ))}
          {message && (
            <div className="rounded-lg border border-line bg-white p-6">
              <p className="text-sm text-neutral-700">{message}</p>
              <Link className="button mt-4" href="/apps/new">Create app</Link>
            </div>
          )}
        </div>
      </section>
    </main>
  );
}

function Info({ label, value, href }: { label: string; value?: string | null; href?: string | null }) {
  return (
    <div className="min-w-0 rounded-md border border-line bg-panel px-3 py-2">
      <div className="text-xs font-medium uppercase tracking-wide text-neutral-500">{label}</div>
      {href ? (
        <a className="mt-1 block truncate text-sm font-medium hover:text-action" href={href} target="_blank" rel="noreferrer">{value || "Not set"}</a>
      ) : (
        <div className="mt-1 truncate text-sm font-medium">{value || "Not set"}</div>
      )}
    </div>
  );
}

function StatusBadge({ status }: { status: string }) {
  const tone = status === "success" ? "bg-emerald-50 text-emerald-800 ring-emerald-200"
    : status === "failed" ? "bg-red-50 text-red-800 ring-red-200"
    : ["running", "building", "starting", "health_checking", "routing", "queued"].includes(status) ? "bg-amber-50 text-amber-800 ring-amber-200"
    : "bg-neutral-100 text-neutral-700 ring-neutral-200";
  return <span className={`rounded-full px-2 py-1 text-xs font-medium ring-1 ${tone}`}>{status.replaceAll("_", " ")}</span>;
}

function ServerBadge({ status }: { status: string }) {
  const online = status === "online";
  return <span className={`rounded-full px-2 py-1 text-xs font-medium ring-1 ${online ? "bg-emerald-50 text-emerald-800 ring-emerald-200" : "bg-neutral-100 text-neutral-700 ring-neutral-200"}`}>machine {status}</span>;
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
  const when = webhook.createdAt ? ` · ${new Date(webhook.createdAt).toLocaleString()}` : "";
  return webhook.ignoredReason ? `${webhook.status}: ${webhook.ignoredReason}` : `${webhook.status}${when}`;
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
