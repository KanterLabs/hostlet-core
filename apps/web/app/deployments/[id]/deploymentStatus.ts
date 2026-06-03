// Shared deployment-status vocabulary and display helpers for the
// deployment detail view. Keeping the ordered step list and the per-status
// help text in one place avoids duplicating the literal status strings
// across the progress UI and the help text.

export const DEPLOYMENT_STEPS = [
  "queued",
  "running",
  "building",
  "starting",
  "health_checking",
  "routing",
  "success",
] as const;

export type DeploymentStep = (typeof DEPLOYMENT_STEPS)[number];

export function humanStatus(status: string) {
  return status.replaceAll("_", " ");
}

export function statusHelp(status: string) {
  switch (status) {
    case "building": return "Hostlet is installing dependencies and building the container image.";
    case "starting": return "The new container is starting. The previous working version is preserved.";
    case "health_checking": return "Hostlet is waiting for the app to answer on the configured port and health path.";
    case "routing": return "The app passed health checks. Hostlet is making it reachable.";
    case "success": return "Deployment succeeded.";
    case "failed": return "Deployment failed. The previous working version was preserved.";
    default: return "Deployment is queued or running.";
  }
}

export function formatDuration(ms?: number | null) {
  if (!ms || ms < 0) return "n/a";
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

export function formatBytes(bytes?: number | null) {
  if (!bytes || bytes < 0) return "n/a";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

export type SocketState = "connecting" | "connected" | "reconnecting" | "closed";

export function socketLabel(state: SocketState) {
  switch (state) {
    case "connected": return "live";
    case "reconnecting": return "reconnecting";
    case "closed": return "closed";
    default: return "connecting";
  }
}
