"use client";

import { use } from "react";
import Link from "next/link";
import { ArrowLeft, CheckCircle2, Clock, RefreshCw, ScrollText, TerminalSquare, XCircle } from "lucide-react";
import { AppShell, DataList, LogViewer, Notice, PageHeader, Panel, SectionHeader, StatusPill, SummaryItem } from "@/components/ui";
import { useDeploymentLogs } from "@/lib/useDeploymentLogs";
import { shortSha } from "@/lib/app-status";
import {
  formatBytes,
  formatDuration,
  humanStatus,
  imageBudgetLabel,
  socketLabel,
  statusHelp,
  statusSteps,
} from "./deploymentStatus";

type RuntimeMetadata = {
  runtime?: string | null;
  packagingStrategy?: string | null;
  buildBackend?: string | null;
  generatedDockerfile?: boolean | null;
  detectedFramework?: string | null;
  webService?: string | null;
  runtimeKind?: string | null;
  packageManager?: string | null;
  gitSyncDurationMs?: number | null;
  buildPlanDurationMs?: number | null;
  buildDurationMs?: number | null;
  imageSizeBytes?: number | null;
  imageBudgetStatus?: string | null;
  imageBudgetWarnBytes?: number | null;
  imageBudgetMaxBytes?: number | null;
  composeUpDurationMs?: number | null;
  containerStartDurationMs?: number | null;
  healthCheckDurationMs?: number | null;
  bootDurationMs?: number | null;
  routingDurationMs?: number | null;
};

type Deployment = {
  id: string;
  appId?: string;
  status: string;
  commitSha?: string | null;
  failure?: string | null;
  runtimeMetadata?: RuntimeMetadata | null;
  queue?: DeploymentQueue | null;
};

type DeploymentQueue = {
  status: "queued" | "building" | "not_applicable";
  position?: number | null;
  deploysAhead?: number | null;
  updatedAt?: string | null;
};

function queueMessage(queue?: DeploymentQueue | null) {
  if (!queue || queue.status === "not_applicable") return null;
  if (queue.status === "building") return "Your app is building now";
  const deploysAhead = Math.max(0, queue.deploysAhead ?? 0);
  if (deploysAhead === 0) return "You're next in line";
  return `${deploysAhead} ${deploysAhead === 1 ? "deploy" : "deploys"} ahead of you`;
}

export default function DeploymentDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const { deployment, logs, socketState, socketMessage } = useDeploymentLogs<Deployment>(id);

  const status = deployment?.status || "loading";
  const finished = status === "success" || status === "failed";
  const steps = statusSteps(status);
  const metadata = deployment?.runtimeMetadata;
  const isCompose = metadata?.runtime === "compose";
  const packaging = isCompose ? "Docker Compose" : metadata?.packagingStrategy || "auto";
  const framework = isCompose ? metadata?.webService ? `service ${metadata.webService}` : "Compose service" : metadata?.detectedFramework || "Repository Dockerfile";
  const queueText = queueMessage(deployment?.queue);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Deployment"
            title="Deployment logs"
            description={deployment?.commitSha ? <>Commit <span className="font-mono">{shortSha(deployment.commitSha)}</span></> : "Build, runtime, health check, and routing output."}
            actions={
              <>
                {deployment?.appId && <Link className="button-secondary" href={`/apps/${deployment.appId}`}><ArrowLeft size={16} />App detail</Link>}
                <Link className="button-secondary" href="/apps"><ScrollText size={16} />All apps</Link>
              </>
            }
          />

          <div className="grid gap-6 xl:grid-cols-[360px_minmax(0,1fr)]">
            <aside className="space-y-6">
              <Panel>
                <SectionHeader icon={ScrollText} title="Status" action={<StatusPill status={status} />} />
                <div className="space-y-3">
                  {steps.map(({ step, done, failed }) => {
                    return (
                      <div key={step} className="flex items-center gap-3 rounded-md bg-surface-alt px-3 py-2">
                        <span className={`flex h-7 w-7 items-center justify-center rounded-full ${failed ? "bg-red-100 text-red-700" : done ? "bg-emerald-100 text-emerald-700" : "bg-surface text-muted ring-1 ring-line"}`}>
                          {failed ? <XCircle size={15} /> : done ? <CheckCircle2 size={15} /> : <Clock size={15} />}
                        </span>
                        <span className="text-sm font-medium">{humanStatus(step)}</span>
                      </div>
                    );
                  })}
                </div>
                <p className="muted mt-4">{statusHelp(status)}</p>
                {queueText && <Notice tone="neutral" className="mt-4" description={queueText} />}
              </Panel>

              {status === "success" && (
                <Notice
                  tone="success"
                  description="Logs from this deploy remain available on this page."
                  action={deployment?.appId && <Link className="button-secondary" href={`/apps/${deployment.appId}`}>Open app detail</Link>}
                />
              )}
              {deployment?.failure && <Notice tone="danger" description={deployment.failure} />}
              {metadata && Object.keys(metadata).length > 0 && (
                <Panel>
                  <SectionHeader title="Deployment metrics" />
                  <DataList className="mt-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-1">
                    <SummaryItem label="Packaging" value={packaging} />
                    <SummaryItem label="Framework" value={framework} />
                    <SummaryItem label="Build backend" value={metadata.buildBackend || (isCompose ? "compose" : "docker")} />
                    <SummaryItem label="Package manager" value={metadata.packageManager || "n/a"} />
                    <SummaryItem label="Git sync" value={formatDuration(metadata.gitSyncDurationMs)} />
                    <SummaryItem label="Build plan" value={formatDuration(metadata.buildPlanDurationMs)} />
                    <SummaryItem label="Build time" value={formatDuration(metadata.buildDurationMs)} />
                    <SummaryItem label="Image size" value={formatBytes(metadata.imageSizeBytes)} />
                    <SummaryItem label="Image budget" value={imageBudgetLabel(metadata.imageBudgetStatus)} />
                    {isCompose && <SummaryItem label="Compose up" value={formatDuration(metadata.composeUpDurationMs)} />}
                    <SummaryItem label={isCompose ? "Startup" : "Container start"} value={formatDuration(metadata.containerStartDurationMs)} />
                    <SummaryItem label="Health wait" value={formatDuration(metadata.healthCheckDurationMs)} />
                    <SummaryItem label="Boot time" value={formatDuration(metadata.bootDurationMs)} />
                    <SummaryItem label="Routing" value={formatDuration(metadata.routingDurationMs)} />
                  </DataList>
                </Panel>
              )}
            </aside>

            <section className="order-first xl:order-none min-w-0">
              <SectionHeader
                icon={TerminalSquare}
                title="Live output"
                className="mb-3"
              />
              {!finished && socketMessage && <Notice tone={socketState === "reconnecting" ? "warning" : "neutral"} className="mb-3" description={socketMessage} />}
              <LogViewer
                lines={logs}
                emptyText="Waiting for deployment logs..."
                highlightFirstError={status === "failed"}
                toolbar={
                  finished
                    ? <span>stream ended</span>
                    : <span className="flex items-center gap-1">{socketState === "reconnecting" && <RefreshCw className="animate-spin" size={14} />}{socketLabel(socketState)}</span>
                }
              />
            </section>
          </div>
    </AppShell>
  );
}
