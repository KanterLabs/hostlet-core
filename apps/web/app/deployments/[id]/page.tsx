"use client";

import { use, useMemo } from "react";
import Link from "next/link";
import { ArrowLeft, CheckCircle2, Clock, RefreshCw, ScrollText, TerminalSquare, XCircle } from "lucide-react";
import { AppShell, DataList, Notice, PageHeader, Panel, SectionHeader, StatusPill, SummaryItem } from "@/components/ui";
import {
  DEPLOYMENT_STEPS,
  formatBytes,
  formatDuration,
  humanStatus,
  socketLabel,
  statusHelp,
} from "./deploymentStatus";
import { useDeploymentLogs } from "./useDeploymentLogs";

export default function DeploymentDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const { deployment, logs, socketState, socketMessage } = useDeploymentLogs(id);

  const status = deployment?.status || "loading";
  const activeIndex = DEPLOYMENT_STEPS.indexOf(status as (typeof DEPLOYMENT_STEPS)[number]);
  const groupedLogs = useMemo(() => logs.join("\n"), [logs]);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Deployment"
            title="Deployment logs"
            description={deployment?.commitSha ? `Commit ${deployment.commitSha}` : "Build, runtime, health check, and routing output."}
            actions={
              <>
                {deployment?.appId && <Link className="button" href={`/apps/${deployment.appId}`}><ArrowLeft size={16} />App detail</Link>}
                <Link className="button-secondary" href="/apps"><ScrollText size={16} />All apps</Link>
              </>
            }
          />

          <div className="grid gap-6 xl:grid-cols-[360px_minmax(0,1fr)]">
            <aside className="space-y-6">
              <Panel>
                <SectionHeader icon={ScrollText} title="Status" action={<StatusPill status={status} />} />
                <div className="space-y-3">
                  {DEPLOYMENT_STEPS.map((step, index) => {
                    const done = status === "failed" ? index < Math.max(activeIndex, 0) : activeIndex >= index;
                    const current = status === step;
                    return (
                      <div key={step} className="flex items-center gap-3 rounded-md bg-surface-alt px-3 py-2">
                        <span className={`flex h-7 w-7 items-center justify-center rounded-full ${status === "failed" && current ? "bg-red-100 text-red-700" : done ? "bg-emerald-100 text-emerald-700" : "bg-surface text-muted ring-1 ring-line"}`}>
                          {status === "failed" && current ? <XCircle size={15} /> : done ? <CheckCircle2 size={15} /> : <Clock size={15} />}
                        </span>
                        <span className="text-sm font-medium">{humanStatus(step)}</span>
                      </div>
                    );
                  })}
                </div>
                <p className="muted mt-4">{statusHelp(status)}</p>
              </Panel>

              {status === "success" && (
                <Notice
                  tone="success"
                  description="Deployment succeeded. Logs remain available here."
                  action={deployment?.appId && <Link className="button-secondary" href={`/apps/${deployment.appId}`}>Open app detail</Link>}
                />
              )}
              {deployment?.failure && <Notice tone="danger" description={deployment.failure} />}
              {deployment?.runtimeMetadata && Object.keys(deployment.runtimeMetadata).length > 0 && (
                <Panel>
                  <SectionHeader title="Build metrics" />
                  <DataList className="mt-4">
                    <SummaryItem label="Packaging" value={deployment.runtimeMetadata.packagingStrategy || "auto"} />
                    <SummaryItem label="Framework" value={deployment.runtimeMetadata.detectedFramework || "Repository Dockerfile"} />
                    <SummaryItem label="Package manager" value={deployment.runtimeMetadata.packageManager || "n/a"} />
                    <SummaryItem label="Build time" value={formatDuration(deployment.runtimeMetadata.buildDurationMs)} />
                    <SummaryItem label="Image size" value={formatBytes(deployment.runtimeMetadata.imageSizeBytes)} />
                  </DataList>
                </Panel>
              )}
              {socketMessage && <Notice tone={socketState === "reconnecting" ? "warning" : "neutral"} description={socketMessage} />}
            </aside>

            <section className="min-w-0">
              <SectionHeader
                icon={TerminalSquare}
                title="Live output"
                className="mb-3"
                action={
                  <div className="flex items-center gap-2 text-xs text-muted">
                    {socketState === "reconnecting" && <RefreshCw className="animate-spin" size={14} />}
                    <span>{socketLabel(socketState)}</span>
                    <span>{logs.length} lines</span>
                  </div>
                }
              />
              <pre className="h-[68vh] max-w-full overflow-auto rounded-lg border border-neutral-800 bg-neutral-950 p-4 text-sm leading-6 text-green-100 shadow-sm shadow-neutral-950/20 [overflow-wrap:normal] [white-space:pre]">
                {groupedLogs || "Waiting for deployment logs..."}
              </pre>
            </section>
          </div>
    </AppShell>
  );
}
