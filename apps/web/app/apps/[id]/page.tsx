"use client";

import { use, useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import {
  Activity,
  AlertTriangle,
  Box,
  Camera,
  Cpu,
  ExternalLink,
  GitBranch,
  Globe2,
  KeyRound,
  Play,
  RotateCcw,
  Save,
  ScrollText,
  Settings,
  Trash2,
} from "lucide-react";
import { api } from "@/lib/api";
import { formatTimestamp } from "@/lib/time";
import { useVisibilityPoll } from "@/lib/useVisibilityPoll";
import { webhookReadiness } from "@/lib/webhooks";
import {
  AppShell,
  DataList,
  Field,
  Metric,
  MetricsGrid,
  Notice,
  PageHeader,
  Panel,
  SectionHeader,
  SelectField,
  StatusPill,
  SummaryItem,
  ToggleCard,
} from "@/components/ui";
import { WebhookNotice } from "@/components/WebhookNotice";
import {
  appVisitHref,
  appVisitLabel,
  cpuDisplay,
  displayDomain,
  diskDisplay,
  healthEventSummary,
  healthMetricDetail,
  isActiveDeploy,
  memoryDisplay,
  networkDisplay,
  pidsDisplay,
  rollbackDisabledReason,
  shortSha,
  webhookSummary,
} from "./appDetailHelpers";
import { emptySettings } from "./appDetail.types";
import type {
  App,
  AppScreenshot,
  ResourceStats,
  RuntimeHealth,
  RuntimeHealthEvent,
  SettingsForm,
} from "./appDetail.types";
import { useAppActions } from "./useAppActions";

export default function AppDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const router = useRouter();
  const [app, setApp] = useState<App | null>(null);
  const [settings, setSettings] = useState<SettingsForm>(emptySettings);
  const [resources, setResources] = useState<ResourceStats | null>(null);
  const [health, setHealth] = useState<RuntimeHealth | null>(null);
  const [healthEvents, setHealthEvents] = useState<RuntimeHealthEvent[]>([]);
  const [screenshot, setScreenshot] = useState<AppScreenshot | null>(null);
  const [screenshotMessage, setScreenshotMessage] = useState("No generated screenshot yet.");
  const [envKeys, setEnvKeys] = useState<Array<{ key: string }>>([]);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [newEnv, setNewEnv] = useState({ key: "", value: "" });
  const [resourceMessage, setResourceMessage] = useState("Waiting for a successful deploy.");

  const refreshScreenshot = useCallback(async () => {
    try {
      const latest = await api<AppScreenshot>(`/api/apps/${id}/screenshots/latest`);
      setScreenshot(latest);
      setScreenshotMessage("");
    } catch {
      setScreenshot(null);
      setScreenshotMessage("No generated screenshot yet.");
    }
  }, [id]);

  const {
    message,
    healthMessage,
    setHealthMessage,
    busyAction,
    refreshApp,
    deploy,
    rollback,
    deleteApp,
    toggleExposure,
    saveSettings,
    saveEnvVar,
    checkHealthNow,
    restartContainer,
    captureScreenshot,
    deleteEnvVar,
  } = useAppActions({
    id,
    app,
    settings,
    router,
    setApp,
    setSettings,
    setHealth,
    setEnvKeys,
    setEnvValues,
    setNewEnv,
    refreshScreenshot,
  });

  useEffect(() => {
    refreshApp();
    refreshScreenshot();
    api<Array<{ key: string }>>(`/api/apps/${id}/env`).then(setEnvKeys).catch(() => setEnvKeys([]));
  }, [id, refreshApp, refreshScreenshot]);

  useVisibilityPoll(
    async ({ isActive }) => {
      try {
        const [snapshot, events] = await Promise.all([
          api<RuntimeHealth>(`/api/apps/${id}/health`),
          api<RuntimeHealthEvent[]>(`/api/apps/${id}/health/events`),
        ]);
        if (!isActive()) return;
        setHealth(snapshot);
        setHealthEvents(events);
        setHealthMessage("");
      } catch (error) {
        if (!isActive()) return;
        setHealth(null);
        setHealthEvents([]);
        setHealthMessage(error instanceof Error ? error.message : "Runtime health is not available yet.");
      }
    },
    { intervalMs: 5000, deps: [id, setHealthMessage] },
  );

  useVisibilityPoll(
    async ({ isActive }) => {
      try {
        const stats = await api<ResourceStats>(`/api/apps/${id}/resources`);
        if (!isActive()) return;
        setResources(stats);
        setResourceMessage("");
      } catch (error) {
        if (!isActive()) return;
        setResources(null);
        setResourceMessage(error instanceof Error ? error.message : "Resource usage is not available yet.");
      }
    },
    { intervalMs: 5000, deps: [id] },
  );

  const deploymentStatus = app?.latestDeployment?.status || (app?.currentDeploymentId ? "success" : "not deployed");
  const active = isActiveDeploy(app?.latestDeployment?.status);
  const cpu = cpuDisplay(resources?.cpuPercent || "0.00%");
  const memory = memoryDisplay(resources);
  const network = networkDisplay(resources);
  const disk = diskDisplay(resources);
  const webhook = webhookReadiness();
  const rollbackReason = rollbackDisabledReason(app, active);
  const visitHref = appVisitHref(app);
  const visitLabel = app ? appVisitLabel(app) : "No route";
  const targetValue = app?.server?.name || "Unknown";
  const targetStatusLabel = `machine ${app?.server?.status || "offline"}`;

  return (
    <AppShell>
          <PageHeader
            eyebrow="Application"
            title={app?.name || "App"}
            description={app ? `${app.repoFullName} · ${app.branch} · ${displayDomain(app.domain)}` : "Loading app..."}
            actions={
              <>
                {visitHref && (
                  <a className="button" href={visitHref} target="_blank" rel="noreferrer"><ExternalLink size={16} />Visit app</a>
                )}
                {app?.latestDeployment?.id && <Link className="button-secondary" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />Logs</Link>}
              </>
            }
          />

          <MetricsGrid>
            <Metric label="Deployment" value={deploymentStatus.replaceAll("_", " ")} detail={shortSha(app?.latestDeployment?.commitSha)} icon={Activity} />
            <Metric label="Runtime health" value={health?.status || "unknown"} detail={healthMetricDetail(health)} icon={AlertTriangle} />
            <Metric label="Machine" value={targetValue} detail={app?.server?.status || "offline"} icon={Box} />
            <Metric label="Exposure" value={app?.publicExposure ? "public" : "private"} detail={visitLabel} icon={Globe2} />
          </MetricsGrid>

          <div className="mb-6 flex flex-wrap gap-2">
            <StatusPill status={deploymentStatus} />
            <StatusPill status={health?.status || "unknown"} label={`health ${health?.status || "unknown"}`} />
            <StatusPill status={app?.server?.status || "offline"} label={targetStatusLabel} />
            <StatusPill status={app?.publicExposure ? "open" : "closed"} label={app?.publicExposure ? "public URL" : "private app"} />
          </div>

          <WebhookNotice autoDeployEnabled={!!app?.autoDeploy} onManualDeploy={deploy} deployDisabled={!!busyAction || active} className="mb-6" />

          <Panel className="mb-6">
            <SectionHeader title="App actions" description="Deploy, operate, publish, and remove this self-hosted app." />
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
              <div>
                <div className="eyebrow mb-2">Deploy</div>
                <div className="flex flex-wrap gap-2">
                  <button className="button" disabled={!!busyAction || active} onClick={deploy}><Play size={16} />{busyAction === "deploy" ? "Starting..." : "Deploy latest"}</button>
                  <button disabled={!!busyAction || !!rollbackReason} title={rollbackReason || "Rollback to the previous successful deployment"} className="button-secondary" onClick={rollback}><RotateCcw size={16} />Rollback</button>
                </div>
              </div>
              <div>
                <div className="eyebrow mb-2">Runtime</div>
                <div className="flex flex-wrap gap-2">
                  <button className="button-secondary" disabled={!!busyAction} onClick={checkHealthNow}>
                    <Activity size={16} />
                    {busyAction === "health" ? "Checking..." : "Check now"}
                  </button>
                  <button className="button-secondary" disabled={!!busyAction || !app?.currentDeploymentId} onClick={restartContainer}>
                    <RotateCcw size={16} />
                    {busyAction === "restart" ? "Restarting..." : "Restart"}
                  </button>
                </div>
              </div>
              <div>
                <div className="eyebrow mb-2">Settings</div>
                {app ? (
                  <button disabled={!!busyAction} className="button-secondary" onClick={toggleExposure}>
                    <Globe2 size={16} />
                    {busyAction === "exposure" ? "Updating..." : app.publicExposure ? "Make private" : "Publish URL"}
                  </button>
                ) : (
                  <p className="muted text-sm">Loading settings.</p>
                )}
              </div>
              <div>
                <div className="eyebrow mb-2">Destructive</div>
                <button disabled={!!busyAction} className="button-danger" onClick={deleteApp}><Trash2 size={16} />Delete</button>
              </div>
            </div>
          </Panel>

          {app && !app.currentDeploymentId && (
            <Notice
              tone="warning"
              className="mb-6"
              title="This app has not been deployed yet."
              description="Start the first deployment to build, run, check health, and publish the route."
              action={<button className="button" disabled={!!busyAction || active} onClick={deploy}><Play size={16} />Start first deployment</button>}
            />
          )}

          {app?.latestDeployment?.status === "failed" && (
            <Notice
              tone="danger"
              className="mb-6"
              title="Latest deployment failed."
              description={app.latestDeployment.failure || "Open the logs to inspect the failure."}
              action={app.latestDeployment.id && <Link className="button-secondary text-red-900 ring-red-200 hover:bg-red-100" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />View failure logs</Link>}
            />
          )}

          {rollbackReason && (
            <Notice
              tone="neutral"
              className="mb-6"
              title="Rollback unavailable."
              description={rollbackReason}
            />
          )}

          <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_390px]">
            <div className="space-y-6">
              <section>
                <SectionHeader
                  title="Runtime health"
                  description="Recurring checks against the current running container."
                />
                {health ? (
                  <Panel>
                    <MetricsGrid columns="md:grid-cols-3" className="mb-0 gap-3">
                      <Metric label="Status" value={health.status} detail={health.lastError || "latest agent check"} />
                      <Metric label="HTTP" value={health.httpStatus ? String(health.httpStatus) : "none"} detail={typeof health.latencyMs === "number" ? `${health.latencyMs} ms` : "no response"} />
                      <Metric label="Failures" value={String(health.failureCount)} detail={health.lastCheckedAt ? `checked ${formatTimestamp(health.lastCheckedAt, "time")}` : "not checked yet"} />
                      <Metric label="Last healthy" value={health.lastHealthyAt ? formatTimestamp(health.lastHealthyAt) : "unknown"} />
                      <Metric label="Container" value={health.containerName || "unknown"} />
                      <Metric label="Target" value={health.checkedUrl || "waiting"} />
                    </MetricsGrid>
                    {healthEvents.length > 0 && (
                      <div className="mt-4 overflow-hidden rounded-md border border-line">
                        {healthEvents.slice(0, 5).map((event) => (
                          <div key={event.id} className="grid gap-2 border-t border-line px-3 py-2 text-sm first:border-t-0 sm:grid-cols-[140px_110px_1fr]">
                            <span className="text-muted">{formatTimestamp(event.createdAt, "time")}</span>
                            <StatusPill status={event.status} />
                            <span className="min-w-0 truncate">{event.error || healthEventSummary(event)}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </Panel>
                ) : (
                  <Notice tone="neutral" description={healthMessage} />
                )}
              </section>

              <section>
                <SectionHeader
                  title="Resource usage"
                  description="Live Docker stats for the current running container."
                  action={resources?.sampledAt && <p className="text-xs text-muted">Updated {formatTimestamp(resources.sampledAt, "time")}</p>}
                />
                {resources ? (
                  <MetricsGrid columns="md:grid-cols-3" className="mb-0 gap-3">
                    <Metric label="CPU" value={cpu.value} detail={cpu.detail} icon={Cpu} />
                    <Metric label="Memory" value={memory.value} detail={memory.detail} />
                    <Metric label="Processes" value={pidsDisplay(resources)} />
                    <Metric label="Network I/O" value={network.value} detail={network.detail} />
                    <Metric label="Disk I/O" value={disk.value} detail={disk.detail} />
                    <Metric label="Container" value={app?.currentDeploymentId ? "running" : "not deployed"} />
                  </MetricsGrid>
                ) : (
                  <Notice tone="neutral" description={resourceMessage} />
                )}
              </section>

              <Panel>
                <SectionHeader icon={Settings} title="App settings" />
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="Domain" value={settings.domain} onChange={(value) => setSettings({ ...settings, domain: value })} />
                  <Field label="Health path" value={settings.health_path} onChange={(value) => setSettings({ ...settings, health_path: value })} />
                  <SelectField label="Runtime" value={settings.runtime_kind} onChange={(value) => setSettings({ ...settings, runtime_kind: value })}>
                    <option value="single">Single service app</option>
                    <option value="compose">Docker Compose</option>
                  </SelectField>
                  {settings.runtime_kind === "compose" && <Field label="Hostlet config" value={settings.hostlet_config_path} onChange={(value) => setSettings({ ...settings, hostlet_config_path: value })} />}
                  <SelectField label="Package with" value={settings.packaging_strategy} onChange={(value) => setSettings({ ...settings, packaging_strategy: value })}>
                    <option value="auto">Auto detect</option>
                    <option value="dockerfile">Repository Dockerfile</option>
                    <option value="generated">Railpack generated runtime</option>
                  </SelectField>
                  <Field label="Root directory" value={settings.root_directory} onChange={(value) => setSettings({ ...settings, root_directory: value })} />
                  <Field label="Container port" type="number" value={settings.container_port} onChange={(value) => setSettings({ ...settings, container_port: value })} />
                  <Field label="Build command" value={settings.build_command} onChange={(value) => setSettings({ ...settings, build_command: value })} />
                  <Field label="Start command" value={settings.start_command} onChange={(value) => setSettings({ ...settings, start_command: value })} />
                  <div className="grid gap-3 sm:grid-cols-2">
                    <SelectField label="Memory" value={settings.memory_limit_mb} onChange={(value) => setSettings({ ...settings, memory_limit_mb: value })}>
                      <option value="">No cap</option>
                      <option value="256">256 MB</option>
                      <option value="512">512 MB</option>
                      <option value="1024">1 GB</option>
                      <option value="2048">2 GB</option>
                      <option value="4096">4 GB</option>
                    </SelectField>
                    <SelectField label="CPU" value={settings.cpu_limit} onChange={(value) => setSettings({ ...settings, cpu_limit: value })}>
                      <option value="">No cap</option>
                      <option value="0.25">0.25 CPU</option>
                      <option value="0.5">0.5 CPU</option>
                      <option value="1">1 CPU</option>
                      <option value="2">2 CPUs</option>
                      <option value="4">4 CPUs</option>
                    </SelectField>
                  </div>
                </div>
                <div className="mt-4 grid gap-3 sm:grid-cols-2">
                  <ToggleCard checked={settings.public_exposure} onChange={(value) => setSettings({ ...settings, public_exposure: value })} icon={Globe2} label="Public URL" />
                  <ToggleCard checked={settings.auto_deploy} onChange={(value) => setSettings({ ...settings, auto_deploy: value })} icon={GitBranch} label="Auto redeploy on branch push" />
                </div>
                <button className="mt-4" disabled={!!busyAction} onClick={saveSettings}><Save size={16} />{busyAction === "settings" ? "Saving..." : "Save settings"}</button>
              </Panel>
            </div>

            <aside className="space-y-6">
              <Panel>
                <SectionHeader icon={KeyRound} title="Environment" />
                <div className="space-y-3">
                  {envKeys.map(({ key }) => (
                    <div key={key} className="rounded-md border border-line p-3">
                      <div className="mb-2 flex items-center justify-between gap-3">
                        <span className="truncate font-mono text-sm">{key}</span>
                        <button className="button-secondary text-red-700 ring-red-200 hover:bg-red-50" disabled={!!busyAction} onClick={() => deleteEnvVar(key)}><Trash2 size={15} />Delete</button>
                      </div>
                      <div className="flex gap-2">
                        <input type="password" value={envValues[key] || ""} onChange={(event) => setEnvValues({ ...envValues, [key]: event.target.value })} placeholder="New value" />
                        <button className="button" disabled={!!busyAction || !envValues[key]} onClick={() => saveEnvVar(key, envValues[key])}>Save</button>
                      </div>
                    </div>
                  ))}
                  {envKeys.length === 0 && <p className="muted">No environment variables set.</p>}
                  <div className="rounded-md border border-line bg-surface-alt p-3">
                    <input value={newEnv.key} onChange={(event) => setNewEnv({ ...newEnv, key: event.target.value.toUpperCase() })} placeholder="KEY" />
                    <input className="mt-2" type="password" value={newEnv.value} onChange={(event) => setNewEnv({ ...newEnv, value: event.target.value })} placeholder="Value" />
                    <button className="button mt-2 w-full" disabled={!!busyAction || !newEnv.key || !newEnv.value} onClick={() => saveEnvVar(newEnv.key, newEnv.value)}><KeyRound size={16} />Add variable</button>
                  </div>
                </div>
              </Panel>

              <Panel>
                <SectionHeader
                  icon={Camera}
                  title="Showcase screenshot"
                  action={
                    <button
                      className="button-secondary"
                      disabled={!!busyAction || !app?.currentDeploymentId || !app?.publicExposure}
                      onClick={captureScreenshot}
                      title={!app?.publicExposure ? "Publish the app URL before capture" : "Capture screenshot"}
                    >
                      <Camera size={16} />
                      {busyAction === "screenshot" ? "Capturing..." : "Capture"}
                    </button>
                  }
                />
                {screenshot ? (
                  <a className="mt-4 block overflow-hidden rounded-md border border-line bg-surface-alt" href={screenshot.publicUrl} target="_blank" rel="noreferrer">
                    <img className="aspect-video w-full object-cover" src={screenshot.publicUrl} alt={`${app?.name || "App"} screenshot`} />
                  </a>
                ) : (
                  <div className="mt-4 flex aspect-video items-center justify-center rounded-md border border-dashed border-line bg-surface-alt text-sm text-muted">
                    {screenshotMessage}
                  </div>
                )}
                <DataList className="mt-4">
                  <SummaryItem label="Captured" value={screenshot?.capturedAt ? formatTimestamp(screenshot.capturedAt) : "waiting"} />
                  <SummaryItem label="Source" value={screenshot?.source || "generated"} />
                </DataList>
              </Panel>

              <Panel>
                <SectionHeader title="Automation" />
                <DataList className="mt-4">
                  <SummaryItem label="Auto redeploy" value={app?.autoDeploy ? "enabled" : "disabled"} />
                  <SummaryItem label="Public URL" value={app?.publicExposure ? "published" : "private"} />
                  <SummaryItem label="Latest webhook" value={webhookSummary(app?.latestWebhook)} />
                </DataList>
                <div className="mt-4 rounded-md border border-line bg-surface-alt p-3 text-sm">
                  <div className="font-medium">GitHub webhook</div>
                  <div className="mt-2 break-all font-mono text-xs">{webhook.webhookUrl}</div>
                </div>
              </Panel>

              {visitHref && (
                <a className="button-secondary w-full" href={visitHref} target="_blank" rel="noreferrer">
                  <ExternalLink size={16} />
                  Open {app?.publicExposure ? "public URL" : "private URL"}
                </a>
              )}
            </aside>
          </div>

          {message && <Notice tone="neutral" className="mt-6" description={message} />}
    </AppShell>
  );
}
