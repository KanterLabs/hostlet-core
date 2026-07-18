"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState } from "react";
import { Box, Camera, ExternalLink, ListFilter, Plus, ScrollText } from "lucide-react";
import { api } from "@/lib/api";
import { formatTimestamp } from "@/lib/time";
import { useVisibilityPoll } from "@/lib/useVisibilityPoll";
import { AppShell, cx, EmptyState, FilterTabs, KeyValueGrid, KeyValueItem, PageHeader, Panel, Skeleton, StatusPill } from "@/components/ui";
import { appVisitHref, appVisitLabel, isActiveDeploy, shortSha } from "./app-links";
import { deploymentSummary, webhookSummary } from "@/lib/app-status";

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
  browser?: {
    status: "pending" | "ready" | "failed" | "skipped";
    checkedAt?: string | null;
    failure?: string | null;
  } | null;
};

// Minimal shape of the per-app screenshot the list needs. The detail page owns
// the full type; the list only reads the public URL and the deployment it came
// from (to flag a stale thumbnail).
type AppScreenshot = {
  deploymentId?: string | null;
  publicUrl: string;
  capturedAt?: string | null;
};

export default function Apps() {
  const [apps, setApps] = useState<App[]>([]);
  const [message, setMessage] = useState("Loading apps...");
  const [filter, setFilter] = useState<"all" | "active" | "failed" | "public" | "healthy" | "degraded" | "unhealthy" | "unknown">("all");
  // undefined = not fetched yet (show skeleton), null = fetched but none, object = latest capture.
  const [screenshots, setScreenshots] = useState<Record<string, AppScreenshot | null>>({});
  const fetchedScreenshots = useRef<Set<string>>(new Set());

  useVisibilityPoll(
    async ({ isActive }) => {
      try {
        const rows = await api<App[]>("/api/apps");
        if (!isActive()) return;
        setApps(rows);
        setMessage(rows.length ? "" : "No apps yet.");
      } catch (e) {
        if (!isActive()) return;
        setMessage(`Could not load apps. ${e instanceof Error ? e.message : "Sign in again."}`);
      }
    },
    { intervalMs: 10000 },
  );
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

  // The list API doesn't embed a screenshot per app, so lazily fetch the latest
  // capture for each deployed app via the same endpoint the detail page uses.
  // Keyed by deployment id so a fresh deploy re-fetches (auto-refreshing the
  // thumbnail); anything missing or failing degrades to a neutral placeholder.
  useEffect(() => {
    let active = true;
    for (const app of apps) {
      if (!app.currentDeploymentId) continue;
      const key = `${app.id}:${app.currentDeploymentId}`;
      if (fetchedScreenshots.current.has(key)) continue;
      fetchedScreenshots.current.add(key);
      api<AppScreenshot>(`/api/apps/${app.id}/screenshots/latest`)
        .then((shot) => {
          if (active) setScreenshots((prev) => ({ ...prev, [app.id]: shot }));
        })
        .catch(() => {
          if (active) setScreenshots((prev) => ({ ...prev, [app.id]: null }));
        });
    }
    return () => {
      active = false;
    };
  }, [apps]);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Applications"
            title="Apps"
            description="Deployable projects, routes, latest health, automation, and public exposure."
            actions={<Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>}
          />

          <FilterTabs label="Filter" icon={ListFilter} value={filter} items={["all", "active", "failed", "public", "healthy", "degraded", "unhealthy", "unknown"] as const} onChange={setFilter} />

          {filtered.length > 0 ? (
            <div className="grid gap-4">
              {filtered.map((app) => {
                const visitHref = appVisitHref(app);
                return (
                <Panel key={app.id} className="overflow-hidden" padded={false}>
                  <div className="flex flex-wrap items-start justify-between gap-4 p-4">
                    <div className="flex min-w-0 flex-1 items-start gap-3">
                      <AppThumbnail app={app} screenshot={screenshots[app.id]} />
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <Link href={`/apps/${app.id}`} className="truncate text-lg font-semibold hover:text-action">{app.name}</Link>
                          <StatusPill status={app.latestDeployment?.status || app.currentDeployment?.status || "not deployed"} />
                          <StatusPill status={app.health?.status || "unknown"} label={`health ${app.health?.status || "unknown"}`} />
                          <StatusPill status={app.server?.status || "offline"} label={`machine ${app.server?.status || "offline"}`} />
                        </div>
                        <p className="muted mt-1 break-all">{app.repoFullName} · {app.branch}</p>
                      </div>
                    </div>
                    <div className="flex shrink-0 flex-wrap gap-2">
                      {visitHref && (
                        <a className="button" href={visitHref} target="_blank" rel="noreferrer"><ExternalLink size={16} />Visit</a>
                      )}
                      <Link className="button-secondary" href={`/apps/${app.id}`}><Box size={16} />Open</Link>
                      {app.latestDeployment?.id && (
                        <Link className="button-secondary" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />Logs</Link>
                      )}
                    </div>
                  </div>

                  <KeyValueGrid>
                    <KeyValueItem label={app.publicExposure ? "Public URL" : "Private URL"} value={appVisitLabel(app)} href={visitHref} externalIcon={<ExternalLink size={13} />} />
                    <KeyValueItem label="Machine" value={`${app.server?.name || "Unknown"} · ${app.server?.kind || "remote"}`} />
                    <KeyValueItem label="Runtime" value={`:${app.containerPort || 3000}${app.healthPath || "/"}`} />
                    <KeyValueItem label="Latest deploy" value={deploymentSummary(app.latestDeployment)} />
                    <KeyValueItem label="Runtime health" value={healthSummary(app.health)} />
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
                );
              })}
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

// Subtle, responsive preview for each row. Mirrors the detail page's dashed
// "No generated screenshot yet" placeholder styling, shows a skeleton while the
// latest capture loads, and flags a thumbnail that predates the current deploy.
function AppThumbnail({ app, screenshot }: { app: App; screenshot?: AppScreenshot | null }) {
  const box = "aspect-video w-24 shrink-0 overflow-hidden rounded-md sm:w-28";

  // Deployed but the latest capture hasn't resolved yet: shimmer in place so the
  // thumbnail doesn't pop from placeholder to image.
  if (app.currentDeploymentId && screenshot === undefined) {
    return <Skeleton className={box} />;
  }

  if (screenshot?.publicUrl) {
    const stale =
      !!app.currentDeploymentId &&
      !!screenshot.deploymentId &&
      screenshot.deploymentId !== app.currentDeploymentId;
    return (
      <Link
        href={`/apps/${app.id}`}
        className={cx(box, "relative block border border-line bg-surface-alt")}
        title={stale ? "Outdated — from a previous deploy" : "Latest screenshot"}
      >
        <img loading="lazy" className="h-full w-full object-cover" src={screenshot.publicUrl} alt={`${app.name} screenshot`} />
        {stale && (
          <span className="absolute inset-x-0 bottom-0 bg-warning-bg px-1 py-0.5 text-center text-[10px] font-medium text-warning-fg">
            Outdated
          </span>
        )}
      </Link>
    );
  }

  return (
    <Link
      href={`/apps/${app.id}`}
      className={cx(box, "flex items-center justify-center border border-dashed border-line bg-surface-alt text-muted")}
      title="No generated screenshot yet."
    >
      <Camera size={16} aria-hidden="true" />
      <span className="sr-only">No generated screenshot yet</span>
    </Link>
  );
}

function healthSummary(health?: RuntimeHealth | null) {
  if (!health) return "unknown";
  const bits = [health.status];
  if (health.browser?.failure) bits.push(health.browser.failure);
  if (health.httpStatus) bits.push(`HTTP ${health.httpStatus}`);
  if (typeof health.latencyMs === "number") bits.push(`${health.latencyMs} ms`);
  if (health.lastCheckedAt) bits.push(`checked ${formatTimestamp(health.lastCheckedAt, "time")}`);
  return bits.join(" · ");
}
