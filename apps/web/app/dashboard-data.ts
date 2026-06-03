// Shared types, constants, and derived-metric helpers for the dashboard overview.

export type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  server?: { name: string; status: string; kind: string } | null;
  latestDeployment?: { id: string; status?: string | null; failure?: string | null; finishedAt?: string | null; startedAt?: string | null } | null;
  health?: { status: string; lastCheckedAt?: string | null } | null;
};

export type Server = { id: string; name: string; kind: string; status: string; lastSeenAt?: string | null };
export type VersionPayload = { currentVersion: string };

// How often the overview re-polls its data while the tab is visible.
export const DASHBOARD_POLL_INTERVAL_MS = 10000;

// Number of apps surfaced in the "Recent apps" preview list.
export const RECENT_APPS_LIMIT = 5;

// Deployment statuses that count as an in-flight ("active") deploy.
const ACTIVE_DEPLOY_STATUSES = ["queued", "running", "building", "starting", "health_checking", "routing"];

export function isActive(status?: string | null) {
  return !!status && ACTIVE_DEPLOY_STATUSES.includes(status);
}

export type DashboardMetrics = {
  appCount: number;
  activeDeploys: number;
  healthyApps: number;
  unhealthyApps: number;
  publicApps: number;
  onlineServers: number;
  serverCount: number;
};

export function deriveMetrics(apps: App[], servers: Server[]): DashboardMetrics {
  return {
    appCount: apps.length,
    activeDeploys: apps.filter((app) => isActive(app.latestDeployment?.status)).length,
    healthyApps: apps.filter((app) => app.health?.status === "healthy").length,
    unhealthyApps: apps.filter((app) => app.health?.status === "unhealthy").length,
    publicApps: apps.filter((app) => app.publicExposure).length,
    onlineServers: servers.filter((server) => server.status === "online").length,
    serverCount: servers.length,
  };
}
