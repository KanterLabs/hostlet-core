// Shared deployment-status vocabulary and display helpers for the
// deployment detail view. Keeping the ordered step list and the per-status
// help text in one place avoids duplicating the literal status strings
// across the progress UI and the help text.

export { formatBytes } from "@/lib/format";
export { formatDuration } from "@/lib/time";

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

export type StepState = {
  step: DeploymentStep;
  done: boolean;
  current: boolean;
  failed: boolean;
};

export function statusSteps(status: string): StepState[] {
  const failed = status === "failed";
  const currentStep = failed ? "success" : status;
  const activeIndex = DEPLOYMENT_STEPS.indexOf(currentStep as DeploymentStep);

  return DEPLOYMENT_STEPS.map((step, index) => ({
    step,
    current: step === currentStep,
    done: failed ? index < DEPLOYMENT_STEPS.length - 1 : activeIndex >= index,
    failed: failed && step === currentStep,
  }));
}

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

export type SocketState = "connecting" | "connected" | "reconnecting" | "closed";

export function socketLabel(state: SocketState) {
  switch (state) {
    case "connected": return "live";
    case "reconnecting": return "reconnecting";
    case "closed": return "closed";
    default: return "connecting";
  }
}

export function imageBudgetLabel(status?: string | null) {
  switch (status) {
    case "ok": return "Within budget";
    case "warning": return "Large";
    case "over_budget": return "Over budget";
    case "unknown": return "Unknown";
    default: return "n/a";
  }
}
