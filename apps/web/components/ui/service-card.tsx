import type { LucideIcon } from "lucide-react";
import { Database, Globe, Layers } from "lucide-react";
import { Panel, SectionHeader } from "@/components/ui/layout";
import { StatusPill } from "@/components/ui/status";
import { Badge } from "@/components/ui/badge";

/// One service of a multi-service (Compose) app, as served on `app.services`.
/// Mirrors the API's `deployment_services` row shape (camelCase).
export type ServiceSummary = {
  name: string;
  /** "web" for the routed entrypoint, "backing" for internal dependencies. */
  role: string;
  containerName?: string | null;
  imageTag?: string | null;
  targetPort?: number | null;
  publishedPort?: number | null;
  /** Container lifecycle state (e.g. running, exited). */
  status?: string | null;
  /** HTTP health classification; only meaningful for the web service. */
  healthStatus?: string | null;
  lastCheckedAt?: string | null;
  lastHealthyAt?: string | null;
};

function serviceIcon(role: string): LucideIcon {
  return role === "web" ? Globe : Database;
}

/** Health is the truth for the web service; backing services report lifecycle. */
function serviceStatus(service: ServiceSummary): string {
  return service.healthStatus || service.status || "unknown";
}

function isUp(service: ServiceSummary): boolean {
  const status = serviceStatus(service);
  return status === "healthy" || status === "running";
}

export function ServiceCard({ service }: { service: ServiceSummary }) {
  const Icon = serviceIcon(service.role);
  const port = service.publishedPort ?? service.targetPort;
  return (
    <div className="flex items-center justify-between gap-3 rounded-lg border border-line bg-surface px-4 py-3">
      <div className="flex min-w-0 items-center gap-3">
        <Icon size={18} className="shrink-0 text-action" />
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate font-medium">{service.name}</span>
            <Badge variant={service.role === "web" ? "neutral" : "outline"}>
              {service.role === "web" ? "web" : "internal"}
            </Badge>
          </div>
          <div className="muted truncate text-xs">
            {service.imageTag || service.containerName || "container"}
          </div>
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {port != null && <Badge variant="outline">:{port}</Badge>}
        <StatusPill status={serviceStatus(service)} />
      </div>
    </div>
  );
}

/**
 * The per-service card stack for a multi-service (Compose) app. The web service
 * is the routed entrypoint; the rest are internal dependencies reachable only on
 * the app's private network. Renders nothing when there are no services.
 */
export function ServiceStack({ services }: { services?: ServiceSummary[] | null }) {
  if (!services || services.length === 0) {
    return null;
  }
  const up = services.filter(isUp).length;
  return (
    <Panel className="mb-6">
      <SectionHeader
        icon={Layers}
        title="Services"
        description="Each container in this multi-service app. The web service is routed publicly; the others are reachable only on the app's internal network."
        action={
          <Badge variant={up === services.length ? "success" : "warning"}>
            {up}/{services.length} up
          </Badge>
        }
      />
      <div className="space-y-2">
        {services.map((service) => (
          <ServiceCard key={service.name} service={service} />
        ))}
      </div>
    </Panel>
  );
}
