import Link from "next/link";
import { Box, GitBranch, HardDrive, Rocket, ShieldCheck } from "lucide-react";
import { GitHubStatus } from "@/components/GitHubStatus";
import { DataList, DataRow, IconFrame, Metric, MetricsGrid, Panel, PanelHeader, SectionHeader, StatusPill } from "@/components/ui";
import type { App, DashboardMetrics, VersionPayload } from "./dashboard-data";

export function OverviewMetrics({ metrics }: { metrics: DashboardMetrics }) {
  return (
    <MetricsGrid>
      <Metric label="Apps" value={String(metrics.appCount)} detail={`${metrics.healthyApps} healthy`} icon={Box} />
      <Metric label="Active deploys" value={String(metrics.activeDeploys)} detail="builds, checks, routing" icon={Rocket} />
      <Metric label="Unhealthy apps" value={String(metrics.unhealthyApps)} detail="runtime monitor" icon={ShieldCheck} />
      <Metric label="Public apps" value={String(metrics.publicApps)} detail="Cloudflare DNS open" icon={ShieldCheck} />
      <Metric label="Machines online" value={`${metrics.onlineServers}/${metrics.serverCount || 1}`} detail="agent heartbeat" icon={HardDrive} />
    </MetricsGrid>
  );
}

function RecentAppRow({ app }: { app: App }) {
  return (
    <Link href={`/apps/${app.id}`} className="grid gap-3 border-t border-line px-4 py-4 first:border-t-0 hover:bg-surface-alt md:grid-cols-[1fr_170px_150px]">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <div className="truncate font-medium">{app.name}</div>
          <StatusPill status={app.latestDeployment?.status || "not deployed"} />
          <StatusPill status={app.health?.status || "unknown"} label={`health ${app.health?.status || "unknown"}`} />
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
  );
}

// Empty-state shown inside the "Recent apps" panel. Mirrors the shared EmptyState
// look, but renders without its own Panel wrapper since it lives inside one already.
function RecentAppsEmpty() {
  return (
    <div className="p-6">
      <div className="flex flex-col items-start">
        <IconFrame icon={Box} className="mb-4" />
        <div className="font-medium">No apps yet</div>
        <p className="muted mt-2 max-w-xl">Create the first app, connect a GitHub repo, then start a deployment.</p>
        <Link className="button mt-5" href="/apps/new">Create app</Link>
      </div>
    </div>
  );
}

export function RecentApps({ apps }: { apps: App[] }) {
  return (
    <Panel className="overflow-hidden" padded={false}>
      <PanelHeader title="Recent apps" description="Latest deployment state by project." action={<Link className="button-secondary" href="/apps">View all</Link>} />
      {apps.length > 0 ? (
        <div>
          {apps.map((app) => (
            <RecentAppRow key={app.id} app={app} />
          ))}
        </div>
      ) : (
        <RecentAppsEmpty />
      )}
    </Panel>
  );
}

export function ReleaseAside({ version }: { version: VersionPayload | null }) {
  return (
    <aside className="space-y-6">
      <GitHubStatus />
      <Panel>
        <SectionHeader icon={GitBranch} title="Release state" />
        <DataList className="mt-4">
          <DataRow label="Version" value={version?.currentVersion || "loading"} />
          <DataRow label="Runtime" value="Docker + Caddy" />
          <DataRow label="Default access" value="Private apps" />
          <DataRow label="CI target" value="self-hosted Linux X64" />
        </DataList>
      </Panel>
    </aside>
  );
}
