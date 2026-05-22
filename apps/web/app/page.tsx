"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Box, GitBranch, HardDrive, Plus, Rocket, ShieldCheck } from "lucide-react";
import { GitHubStatus } from "@/components/GitHubStatus";
import { api } from "@/lib/api";
import { AppShell, DataList, DataRow, IconFrame, Metric, MetricsGrid, Notice, PageHeader, Panel, PanelHeader, SectionHeader, StatusPill } from "@/components/ui";

type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  server?: { name: string; status: string; kind: string } | null;
  latestDeployment?: { id: string; status?: string | null; failure?: string | null; finishedAt?: string | null; startedAt?: string | null } | null;
};

type Server = { id: string; name: string; kind: string; status: string; lastSeenAt?: string | null };

export default function Dashboard() {
  const [apps, setApps] = useState<App[]>([]);
  const [servers, setServers] = useState<Server[]>([]);
  const [message, setMessage] = useState("Loading Hostlet...");

  useEffect(() => {
    Promise.all([
      api<App[]>("/api/apps"),
      api<Server[]>("/api/servers"),
    ])
      .then(([appRows, serverRows]) => {
        setApps(appRows);
        setServers(serverRows);
        setMessage("");
      })
      .catch((err) => setMessage(err instanceof Error ? err.message : "Could not load Hostlet."));
  }, []);

  const activeDeploys = apps.filter((app) => isActive(app.latestDeployment?.status)).length;
  const healthyApps = apps.filter((app) => app.latestDeployment?.status === "success").length;
  const publicApps = apps.filter((app) => app.publicExposure).length;
  const onlineServers = servers.filter((server) => server.status === "online").length;
  const recentApps = useMemo(() => apps.slice(0, 5), [apps]);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Control plane"
            title="Overview"
            description="Deploy GitHub projects onto your own machines with Docker, Caddy, live logs, rollbacks, and optional Cloudflare exposure."
            actions={
              <Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>
            }
          />

          <MetricsGrid>
            <Metric label="Apps" value={String(apps.length)} detail={`${healthyApps} healthy`} icon={Box} />
            <Metric label="Active deploys" value={String(activeDeploys)} detail="builds, checks, routing" icon={Rocket} />
            <Metric label="Public apps" value={String(publicApps)} detail="Cloudflare DNS open" icon={ShieldCheck} />
            <Metric label="Machines online" value={`${onlineServers}/${servers.length || 1}`} detail="agent heartbeat" icon={HardDrive} />
          </MetricsGrid>

          <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_380px]">
            <Panel className="overflow-hidden" padded={false}>
              <PanelHeader title="Recent apps" description="Latest deployment state by project." action={<Link className="button-secondary" href="/apps">View all</Link>} />
              {recentApps.length > 0 ? (
                <div>
                  {recentApps.map((app) => (
                    <Link key={app.id} href={`/apps/${app.id}`} className="grid gap-3 border-t border-line px-4 py-4 first:border-t-0 hover:bg-surface-alt md:grid-cols-[1fr_170px_150px]">
                      <div className="min-w-0">
                        <div className="flex items-center gap-2">
                          <div className="truncate font-medium">{app.name}</div>
                          <StatusPill status={app.latestDeployment?.status || "not deployed"} />
                        </div>
                        <p className="muted mt-1 truncate">{app.repoFullName} · {app.branch}</p>
                      </div>
                      <div className="text-sm">
                        <div className="eyebrow">Machine</div>
                        <div className="mt-1 truncate">{app.server?.name || "Unknown"}</div>
                      </div>
                      <div className="text-sm">
                        <div className="eyebrow">Exposure</div>
                        <div className="mt-1">{app.publicExposure ? "Public" : "Private"}</div>
                      </div>
                    </Link>
                  ))}
                </div>
              ) : (
                <div className="p-6">
                  <div className="flex flex-col items-start">
                    <IconFrame icon={Box} className="mb-4" />
                    <div className="font-medium">No apps yet</div>
                    <p className="muted mt-2 max-w-xl">Create the first app, connect a GitHub repo, then start a deployment.</p>
                    <Link className="button mt-5" href="/apps/new">Create app</Link>
                  </div>
                </div>
              )}
            </Panel>

            <aside className="space-y-6">
              <GitHubStatus />
              <Panel>
                <SectionHeader icon={GitBranch} title="Release state" />
                <DataList className="mt-4">
                  <DataRow label="Version" value="0.1.0" />
                  <DataRow label="Runtime" value="Docker + Caddy" />
                  <DataRow label="Default access" value="Private apps" />
                  <DataRow label="CI target" value="self-hosted Linux X64" />
                </DataList>
              </Panel>
            </aside>
          </div>

          {message && (
            <Notice tone="warning" className="mt-6" description={message} />
          )}
    </AppShell>
  );
}

function isActive(status?: string | null) {
  return !!status && ["queued", "running", "building", "starting", "health_checking", "routing"].includes(status);
}
