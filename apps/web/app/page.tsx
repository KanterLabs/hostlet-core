"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Plus } from "lucide-react";
import { api } from "@/lib/api";
import { useVisibilityPoll } from "@/lib/useVisibilityPoll";
import { AppShell, Notice, PageHeader } from "@/components/ui";
import {
  DASHBOARD_POLL_INTERVAL_MS,
  RECENT_APPS_LIMIT,
  deriveMetrics,
  type App,
  type Server,
  type VersionPayload,
} from "./dashboard-data";
import { OverviewMetrics, RecentApps, ReleaseAside } from "./dashboard-sections";

export default function Dashboard() {
  const [apps, setApps] = useState<App[]>([]);
  const [servers, setServers] = useState<Server[]>([]);
  const [version, setVersion] = useState<VersionPayload | null>(null);
  const [message, setMessage] = useState("Loading Hostlet...");

  useVisibilityPoll(
    async ({ isActive }) => {
      try {
        const [appRows, serverRows] = await Promise.all([
          api<App[]>("/api/apps"),
          api<Server[]>("/api/servers"),
        ]);
        if (!isActive()) return;
        setApps(appRows);
        setServers(serverRows);
        setMessage("");
      } catch (err) {
        if (!isActive()) return;
        setMessage(err instanceof Error ? err.message : "Could not load Hostlet.");
      }
    },
    { intervalMs: DASHBOARD_POLL_INTERVAL_MS },
  );

  useEffect(() => {
    let active = true;
    api<VersionPayload>("/api/system/version")
      .then((payload) => {
        if (active) setVersion(payload);
      })
      .catch(() => {
        if (active) setVersion(null);
      });
    return () => {
      active = false;
    };
  }, []);

  const metrics = useMemo(() => deriveMetrics(apps, servers), [apps, servers]);
  const recentApps = useMemo(() => apps.slice(0, RECENT_APPS_LIMIT), [apps]);

  return (
    <AppShell>
      <PageHeader
        eyebrow="Control plane"
        title="Overview"
        description="Deploy GitHub projects onto your own machines with Docker, Caddy, live logs, rollbacks, and optional Cloudflare exposure."
        actions={<Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>}
      />

      <OverviewMetrics metrics={metrics} />

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_380px]">
        <RecentApps apps={recentApps} />
        <ReleaseAside version={version} />
      </div>

      {message && (
        <Notice tone="warning" className="mt-6" description={message} />
      )}
    </AppShell>
  );
}
