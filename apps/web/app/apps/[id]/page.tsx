"use client";

import { use, useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import {
  Activity,
  AlertTriangle,
  Box,
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

type ResourceStats = {
  cpuPercent: string;
  memoryUsage: string;
  memoryPercent: string;
  networkIo: string;
  blockIo: string;
  pids: string;
  sampledAt: string;
};

type RuntimeHealth = {
  appId?: string;
  deploymentId?: string | null;
  containerName?: string | null;
  status: string;
  checkedUrl?: string | null;
  httpStatus?: number | null;
  latencyMs?: number | null;
  failureCount: number;
  successCount: number;
  lastError?: string | null;
  lastCheckedAt?: string | null;
  lastHealthyAt?: string | null;
  updatedAt?: string | null;
};

type RuntimeHealthEvent = {
  id: string;
  status: string;
  httpStatus?: number | null;
  latencyMs?: number | null;
  error?: string | null;
  createdAt: string;
};

type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  containerPort?: number | null;
  healthPath?: string | null;
  runtimeKind?: string | null;
  hostletConfigPath?: string | null;
  rootDirectory?: string | null;
  installCommand?: string | null;
  buildCommand?: string | null;
  startCommand?: string | null;
  memoryLimitMb?: number | null;
  cpuLimit?: number | null;
  currentDeploymentId?: string | null;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  server?: { id: string; name: string; kind: string; status: string; publicIp?: string | null; lastSeenAt?: string | null } | null;
  latestDeployment?: { id: string; status?: string | null; failure?: string | null; commitSha?: string | null; startedAt?: string | null; finishedAt?: string | null } | null;
  currentDeployment?: { status: string; publishedPort?: number | null; finishedAt?: string | null } | null;
  latestWebhook?: {
    status: string;
    ignoredReason?: string | null;
    commitSha?: string | null;
    branch?: string | null;
    createdAt?: string | null;
  } | null;
  health?: RuntimeHealth | null;
};

type AgentJob = {
  id: string;
  status: "queued" | "running" | "success" | "failed";
  failure?: string | null;
};

type SettingsForm = {
  domain: string;
  health_path: string;
  runtime_kind: string;
  hostlet_config_path: string;
  root_directory: string;
  install_command: string;
  build_command: string;
  start_command: string;
  container_port: string;
  memory_limit_mb: string;
  cpu_limit: string;
  public_exposure: boolean;
  auto_deploy: boolean;
};

type SessionPayload = {
  mode: "self_hosted" | "cloud";
  authenticated: boolean;
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
  } | null;
};

const emptySettings: SettingsForm = {
  domain: "",
  health_path: "/",
  runtime_kind: "single",
  hostlet_config_path: "hostlet.yml",
  root_directory: ".",
  install_command: "",
  build_command: "",
  start_command: "",
  container_port: "3000",
  memory_limit_mb: "",
  cpu_limit: "",
  public_exposure: false,
  auto_deploy: false,
};

export default function AppDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const router = useRouter();
  const [app, setApp] = useState<App | null>(null);
  const [session, setSession] = useState<SessionPayload | null>(null);
  const [settings, setSettings] = useState<SettingsForm>(emptySettings);
  const [resources, setResources] = useState<ResourceStats | null>(null);
  const [health, setHealth] = useState<RuntimeHealth | null>(null);
  const [healthEvents, setHealthEvents] = useState<RuntimeHealthEvent[]>([]);
  const [envKeys, setEnvKeys] = useState<Array<{ key: string }>>([]);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [newEnv, setNewEnv] = useState({ key: "", value: "" });
  const [resourceMessage, setResourceMessage] = useState("Waiting for a successful deploy.");
  const [healthMessage, setHealthMessage] = useState("Waiting for runtime health.");
  const [message, setMessage] = useState("");
  const [busyAction, setBusyAction] = useState<"deploy" | "rollback" | "exposure" | "delete" | "settings" | "env" | "health" | "restart" | "">("");

  useEffect(() => {
    refreshApp();
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
    api<Array<{ key: string }>>(`/api/apps/${id}/env`).then(setEnvKeys).catch(() => setEnvKeys([]));
  }, [id]);

  useEffect(() => {
    let active = true;
    async function loadHealth() {
      try {
        const [snapshot, events] = await Promise.all([
          api<RuntimeHealth>(`/api/apps/${id}/health`),
          api<RuntimeHealthEvent[]>(`/api/apps/${id}/health/events`),
        ]);
        if (!active) return;
        setHealth(snapshot);
        setHealthEvents(events);
        setHealthMessage("");
      } catch (error) {
        if (!active) return;
        setHealth(null);
        setHealthEvents([]);
        setHealthMessage(error instanceof Error ? error.message : "Runtime health is not available yet.");
      }
    }
    loadHealth();
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") loadHealth();
    }, 5000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [id]);

  useEffect(() => {
    let active = true;
    async function loadResources() {
      try {
        const stats = await api<ResourceStats>(`/api/apps/${id}/resources`);
        if (!active) return;
        setResources(stats);
        setResourceMessage("");
      } catch (error) {
        if (!active) return;
        setResources(null);
        setResourceMessage(error instanceof Error ? error.message : "Resource usage is not available yet.");
      }
    }
    loadResources();
    const timer = setInterval(loadResources, 5000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [id]);

  async function refreshApp() {
    try {
      const loaded = await api<App>(`/api/apps/${id}`);
      setApp(loaded);
      if (loaded.health) setHealth(loaded.health);
      setSettings({
        domain: loaded.domain || "",
        health_path: loaded.healthPath || "/",
        runtime_kind: loaded.runtimeKind || "single",
        hostlet_config_path: loaded.hostletConfigPath || "hostlet.yml",
        root_directory: loaded.rootDirectory || ".",
        install_command: loaded.installCommand || "",
        build_command: loaded.buildCommand || "",
        start_command: loaded.startCommand || "",
        container_port: String(loaded.containerPort || 3000),
        memory_limit_mb: loaded.memoryLimitMb ? String(loaded.memoryLimitMb) : "",
        cpu_limit: loaded.cpuLimit ? String(loaded.cpuLimit) : "",
        public_exposure: !!loaded.publicExposure,
        auto_deploy: !!loaded.autoDeploy,
      });
    } catch {
      setMessage("Could not load app. Sign in and check that it still exists.");
    }
  }

  async function deploy() {
    if (busyAction || isActiveDeploy(app?.latestDeployment?.status)) return;
      setBusyAction("deploy");
    setMessage("Starting deployment...");
    try {
      const res = await api<{ deploymentId: string }>(`/api/apps/${id}/deploy`, { method: "POST", body: "{}" });
      router.push(`/deployments/${res.deploymentId}`);
    } catch (error) {
      setMessage(`Deploy failed to start. ${error instanceof Error ? error.message : ""}`);
      setBusyAction("");
    }
  }

  async function rollback() {
    if (busyAction) return;
      setBusyAction("rollback");
    setMessage("Starting rollback...");
    try {
      const res = await api<{ rollbackDeploymentId: string }>(`/api/apps/${id}/rollback`, { method: "POST", body: "{}" });
      router.push(`/deployments/${res.rollbackDeploymentId}`);
    } catch (error) {
      setMessage(`Rollback could not start. ${error instanceof Error ? error.message : ""}`);
      setBusyAction("");
    }
  }

  async function deleteApp() {
    if (!confirm("Delete this app, its Hostlet-managed route, containers, images, and deployment history?")) return;
    if (busyAction) return;
    setBusyAction("delete");
    setMessage("Deleting app and requesting server cleanup...");
    try {
      const result = await api<{ jobId?: string } | undefined>(`/api/apps/${id}`, { method: "DELETE" });
      if (result?.jobId) {
        setMessage("Server cleanup is running...");
        await waitForAgentJob(result.jobId, setMessage);
      }
      router.push("/apps");
    } catch (error) {
      setMessage(`Delete failed. ${error instanceof Error ? error.message : ""}`);
      setBusyAction("");
    }
  }

  async function toggleExposure() {
    if (!app || busyAction) return;
    const next = !app.publicExposure;
    setBusyAction("exposure");
    setMessage(next ? "Publishing app URL..." : "Making app private...");
    try {
      await api(`/api/apps/${id}`, { method: "PATCH", body: JSON.stringify({ public_exposure: next }) });
      await refreshApp();
      setMessage(next ? "App URL published. DNS may take a moment to propagate." : "App URL is private.");
    } catch (error) {
      setMessage(`${next ? "Publish" : "Unpublish"} failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  async function saveSettings() {
    if (busyAction) return;
    setBusyAction("settings");
    setMessage("Saving app settings...");
    try {
      const payload: Record<string, unknown> = {
        health_path: settings.health_path,
        root_directory: settings.root_directory || ".",
        install_command: settings.install_command.trim() || null,
        build_command: settings.build_command.trim() || null,
        start_command: settings.start_command.trim() || null,
        container_port: Number(settings.container_port),
      };
      if (!cloud) {
        payload.domain = settings.domain;
        payload.runtime_kind = settings.runtime_kind;
        payload.hostlet_config_path = settings.hostlet_config_path || "hostlet.yml";
        payload.memory_limit_mb = settings.memory_limit_mb ? Number(settings.memory_limit_mb) : null;
        payload.cpu_limit = settings.cpu_limit ? Number(settings.cpu_limit) : null;
        payload.public_exposure = settings.public_exposure;
        payload.auto_deploy = settings.auto_deploy;
      }
      await api(`/api/apps/${id}`, {
        method: "PATCH",
        body: JSON.stringify(payload),
      });
      await refreshApp();
      setMessage("Settings saved. Redeploy for runtime changes to reach the container.");
    } catch (error) {
      setMessage(`Save failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  async function saveEnvVar(key: string, value: string) {
    if (busyAction || !key.trim()) return;
    setBusyAction("env");
    setMessage("Saving environment variable...");
    try {
      await api(`/api/apps/${id}/env/${encodeURIComponent(key.trim().toUpperCase())}`, {
        method: "PUT",
        body: JSON.stringify({ value }),
      });
      setEnvKeys(await api<Array<{ key: string }>>(`/api/apps/${id}/env`));
      setEnvValues((current) => ({ ...current, [key]: "" }));
      setNewEnv({ key: "", value: "" });
      setMessage("Environment variable saved. Redeploy for the change to reach the container.");
    } catch (error) {
      setMessage(`Env save failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  async function checkHealthNow() {
    if (busyAction) return;
    setBusyAction("health");
    setHealthMessage("Requesting a fresh health check...");
    try {
      await api(`/api/apps/${id}/health/check-now`, { method: "POST", body: "{}" });
      setHealthMessage("Health check requested. Waiting for the agent result...");
    } catch (error) {
      setHealthMessage(`Health check could not start. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  async function restartContainer() {
    if (busyAction || !confirm("Restart the current app container?")) return;
    setBusyAction("restart");
    setHealthMessage("Requesting container restart...");
    try {
      await api(`/api/apps/${id}/restart`, { method: "POST", body: "{}" });
      setHealthMessage("Container restart requested. Waiting for the agent health result...");
    } catch (error) {
      setHealthMessage(`Restart could not start. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  async function deleteEnvVar(key: string) {
    if (busyAction || !confirm(`Delete ${key}?`)) return;
    setBusyAction("env");
    setMessage("Deleting environment variable...");
    try {
      await api(`/api/apps/${id}/env/${encodeURIComponent(key)}`, { method: "DELETE" });
      setEnvKeys(await api<Array<{ key: string }>>(`/api/apps/${id}/env`));
      setMessage("Environment variable deleted. Redeploy for the change to reach the container.");
    } catch (error) {
      setMessage(`Env delete failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  const deploymentStatus = app?.latestDeployment?.status || (app?.currentDeploymentId ? "success" : "not deployed");
  const active = isActiveDeploy(app?.latestDeployment?.status);
  const cpu = cpuDisplay(resources?.cpuPercent || "0.00%");
  const webhook = webhookReadiness();
  const rollbackReason = rollbackDisabledReason(app, active);
  const cloud = session?.mode === "cloud";
  const visitHref = appVisitHref(app, cloud);
  const visitLabel = app ? appVisitLabel(app, cloud) : "No route";
  const targetLabel = cloud ? "Worker" : "Machine";
  const targetValue = cloud ? "Hostlet Cloud" : app?.server?.name || "Unknown";
  const targetStatusLabel = `${cloud ? "worker" : "machine"} ${app?.server?.status || "offline"}`;

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
            <Metric label={targetLabel} value={targetValue} detail={app?.server?.status || "offline"} icon={Box} />
            <Metric label={cloud ? "Cloud URL" : "Exposure"} value={cloud ? "managed" : app?.publicExposure ? "public" : "private"} detail={visitLabel} icon={Globe2} />
          </MetricsGrid>

          <div className="mb-6 flex flex-wrap gap-2">
            <StatusPill status={deploymentStatus} />
            <StatusPill status={health?.status || "unknown"} label={`health ${health?.status || "unknown"}`} />
            <StatusPill status={app?.server?.status || "offline"} label={targetStatusLabel} />
            <StatusPill status={app?.publicExposure ? "open" : "closed"} label={cloud ? "Hostlet Cloud URL" : app?.publicExposure ? "public URL" : "private app"} />
          </div>

          {!cloud && <WebhookNotice autoDeployEnabled={!!app?.autoDeploy} onManualDeploy={deploy} deployDisabled={!!busyAction || active} className="mb-6" />}

          <Panel className="mb-6">
            <SectionHeader title="App actions" description={cloud ? "Deploy and operate this app on the managed Hostlet Cloud worker." : "Deploy, operate, publish, and remove this self-hosted app."} />
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
              <div>
                <div className="eyebrow mb-2">Deploy</div>
                <div className="flex flex-wrap gap-2">
                  <button disabled={!!busyAction || active} onClick={deploy}><Play size={16} />{busyAction === "deploy" ? "Starting..." : "Deploy latest"}</button>
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
                {cloud ? (
                  <p className="muted text-sm">Cloud URL, exposure, and runtime kind are managed by Hostlet Cloud.</p>
                ) : app ? (
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
              action={<button disabled={!!busyAction || active} onClick={deploy}><Play size={16} />Start first deployment</button>}
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
                      <Metric label="Failures" value={String(health.failureCount)} detail={health.lastCheckedAt ? `checked ${new Date(health.lastCheckedAt).toLocaleTimeString()}` : "not checked yet"} />
                      <Metric label="Last healthy" value={health.lastHealthyAt ? new Date(health.lastHealthyAt).toLocaleString() : "unknown"} />
                      <Metric label="Container" value={health.containerName || "unknown"} />
                      <Metric label="Target" value={health.checkedUrl || "waiting"} />
                    </MetricsGrid>
                    {healthEvents.length > 0 && (
                      <div className="mt-4 overflow-hidden rounded-md border border-line">
                        {healthEvents.slice(0, 5).map((event) => (
                          <div key={event.id} className="grid gap-2 border-t border-line px-3 py-2 text-sm first:border-t-0 sm:grid-cols-[140px_110px_1fr]">
                            <span className="text-muted">{new Date(event.createdAt).toLocaleTimeString()}</span>
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
                  action={resources?.sampledAt && <p className="text-xs text-muted">Updated {new Date(resources.sampledAt).toLocaleTimeString()}</p>}
                />
                {resources ? (
                  <MetricsGrid columns="md:grid-cols-3" className="mb-0 gap-3">
                    <Metric label="CPU" value={cpu.value} detail={cpu.detail} icon={Cpu} />
                    <Metric label="Memory" value={resources.memoryUsage} detail={resources.memoryPercent} />
                    <Metric label="Processes" value={resources.pids} />
                    <Metric label="Network I/O" value={resources.networkIo} />
                    <Metric label="Disk I/O" value={resources.blockIo} />
                    <Metric label="Container" value={app?.currentDeploymentId ? "running" : "not deployed"} />
                  </MetricsGrid>
                ) : (
                  <Notice tone="neutral" description={resourceMessage} />
                )}
              </section>

              <Panel>
                <SectionHeader icon={Settings} title="App settings" description={cloud ? "Cloud apps keep managed URL, runtime, and exposure settings managed by Hostlet." : undefined} />
                {cloud && (
                  <Notice
                    tone="neutral"
                    className="mb-4"
                    title="Managed cloud settings"
                    description="Hostlet Cloud assigns the URL, keeps apps publicly reachable at that URL, and uses the single-service runtime."
                  />
                )}
                <div className="grid gap-4 md:grid-cols-2">
                  {!cloud && <Field label="Domain" value={settings.domain} onChange={(value) => setSettings({ ...settings, domain: value })} />}
                  {cloud && <SummaryItem label="Hostlet Cloud URL" value={displayDomain(settings.domain)} />}
                  <Field label="Health path" value={settings.health_path} onChange={(value) => setSettings({ ...settings, health_path: value })} />
                  {!cloud && (
                    <SelectField label="Runtime" value={settings.runtime_kind} onChange={(value) => setSettings({ ...settings, runtime_kind: value })}>
                      <option value="single">Dockerfile or Node</option>
                      <option value="compose">Docker Compose</option>
                    </SelectField>
                  )}
                  {cloud && <SummaryItem label="Runtime" value="Single-service Dockerfile or generated Node app" />}
                  {!cloud && settings.runtime_kind === "compose" && <Field label="Hostlet config" value={settings.hostlet_config_path} onChange={(value) => setSettings({ ...settings, hostlet_config_path: value })} />}
                  <Field label="Root directory" value={settings.root_directory} onChange={(value) => setSettings({ ...settings, root_directory: value })} />
                  <Field label="Container port" type="number" value={settings.container_port} onChange={(value) => setSettings({ ...settings, container_port: value })} />
                  <Field label="Install command" value={settings.install_command} onChange={(value) => setSettings({ ...settings, install_command: value })} />
                  <Field label="Build command" value={settings.build_command} onChange={(value) => setSettings({ ...settings, build_command: value })} />
                  <Field label="Start command" value={settings.start_command} onChange={(value) => setSettings({ ...settings, start_command: value })} />
                  {!cloud && (
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
                  )}
                </div>
                {!cloud && (
                  <div className="mt-4 grid gap-3 sm:grid-cols-2">
                    <ToggleCard checked={settings.public_exposure} onChange={(value) => setSettings({ ...settings, public_exposure: value })} icon={Globe2} label="Public URL" />
                    <ToggleCard checked={settings.auto_deploy} onChange={(value) => setSettings({ ...settings, auto_deploy: value })} icon={GitBranch} label="Auto redeploy on branch push" />
                  </div>
                )}
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
                        <button disabled={!!busyAction || !envValues[key]} onClick={() => saveEnvVar(key, envValues[key])}>Save</button>
                      </div>
                    </div>
                  ))}
                  {envKeys.length === 0 && <p className="muted">No environment variables set.</p>}
                  <div className="rounded-md border border-line bg-surface-alt p-3">
                    <input value={newEnv.key} onChange={(event) => setNewEnv({ ...newEnv, key: event.target.value.toUpperCase() })} placeholder="KEY" />
                    <input className="mt-2" type="password" value={newEnv.value} onChange={(event) => setNewEnv({ ...newEnv, value: event.target.value })} placeholder="Value" />
                    <button className="mt-2 w-full" disabled={!!busyAction || !newEnv.key || !newEnv.value} onClick={() => saveEnvVar(newEnv.key, newEnv.value)}><KeyRound size={16} />Add variable</button>
                  </div>
                </div>
              </Panel>

              {!cloud && (
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
              )}

              {visitHref && (
                <a className="button-secondary w-full" href={visitHref} target="_blank" rel="noreferrer">
                  <ExternalLink size={16} />
                  Open {cloud ? "Hostlet Cloud URL" : app?.publicExposure ? "public URL" : "private URL"}
                </a>
              )}
            </aside>
          </div>

          {message && <Notice tone="neutral" className="mt-6" description={message} />}
    </AppShell>
  );
}

function webhookSummary(webhook?: App["latestWebhook"] | null) {
  if (!webhook) return "No push seen";
  const sha = webhook.commitSha ? ` ${webhook.commitSha.slice(0, 7)}` : "";
  return webhook.ignoredReason ? `ignored${sha}: ${webhook.ignoredReason}` : `${webhook.status}${sha}`;
}

function isActiveDeploy(status?: string | null) {
  return !!status && ["queued", "running", "building", "starting", "health_checking", "routing"].includes(status);
}

function rollbackDisabledReason(app: App | null, active: boolean) {
  if (!app) return "App details are still loading.";
  if (active) return "Wait for the active deployment to finish before rolling back.";
  if (app.runtimeKind === "compose") return "Compose rollback is disabled for Hostlet 0.4.0. Redeploy the target revision instead.";
  if (!app.currentDeploymentId) return "Deploy this app once before rolling back.";
  return "";
}

function shortSha(sha?: string | null) {
  if (!sha || sha === "HEAD") return sha || "No deploy yet";
  return sha.slice(0, 7);
}

function cpuDisplay(raw: string) {
  const value = Number.parseFloat(raw.replace("%", ""));
  if (Number.isFinite(value) && value <= 0) {
    return { value: "Idle", detail: `${raw} CPU` };
  }
  if (Number.isFinite(value) && value > 0 && value < 0.01) {
    return { value: "<0.01%", detail: `${raw} CPU` };
  }
  return { value: raw, detail: "Docker live sample" };
}

function healthMetricDetail(health?: RuntimeHealth | null) {
  if (!health) return "waiting for agent";
  if (health.lastError) return health.lastError;
  if (typeof health.latencyMs === "number") return `${health.latencyMs} ms`;
  return health.lastCheckedAt ? `checked ${new Date(health.lastCheckedAt).toLocaleTimeString()}` : "not checked yet";
}

function healthEventSummary(event: RuntimeHealthEvent) {
  const bits = [];
  if (event.httpStatus) bits.push(`HTTP ${event.httpStatus}`);
  if (typeof event.latencyMs === "number") bits.push(`${event.latencyMs} ms`);
  return bits.length ? bits.join(" · ") : "no response data";
}

async function waitForAgentJob(jobId: string, setMessage: (message: string) => void) {
  for (let attempt = 1; attempt <= 60; attempt += 1) {
    const job = await api<AgentJob>(`/api/agent-jobs/${jobId}`);
    if (job.status === "success") return;
    if (job.status === "failed") {
      throw new Error(job.failure || "Server cleanup failed.");
    }
    setMessage(`Server cleanup is ${job.status}. Waiting for confirmation...`);
    await new Promise((resolve) => setTimeout(resolve, 2000));
  }
  throw new Error("Server cleanup did not finish within 120 seconds.");
}

function displayDomain(domain: string) {
  if (!domain || typeof window === "undefined") return domain;
  try {
    const withProtocol = domain.startsWith("http://") || domain.startsWith("https://") ? domain : `http://${domain}`;
    const url = new URL(withProtocol);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1") {
      url.hostname = window.location.hostname;
      return url.host + url.pathname.replace(/\/$/, "");
    }
  } catch {
    return domain;
  }
  return domain;
}

function appVisitHref(app?: App | null, cloud = false) {
  if (!app?.currentDeploymentId) return null;
  if (cloud) {
    const display = displayDomain(app.domain);
    if (!display) return null;
    return display.startsWith("http://") || display.startsWith("https://") ? display : `https://${display}`;
  }
  if (!app.publicExposure) {
    const port = app.currentDeployment?.publishedPort;
    const host = privateAppHost(app);
    return port && host ? `http://${host}:${port}` : null;
  }
  const display = displayDomain(app.domain);
  if (!display) return null;
  if (display.startsWith("http://") || display.startsWith("https://")) return display;
  try {
    const url = new URL(`http://${display}`);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1" || /^[\d.]+$/.test(url.hostname)) {
      return `http://${display}`;
    }
  } catch {
    return null;
  }
  return `https://${display}`;
}

function appVisitLabel(app: App, cloud = false) {
  if (cloud) return displayDomain(app.domain) || "No Hostlet Cloud URL";
  if (app.publicExposure) return displayDomain(app.domain) || "No public URL";
  const port = app.currentDeployment?.publishedPort;
  const host = privateAppHost(app);
  return port && host ? `${host}:${port}` : "Deploy to assign a private port";
}

function privateAppHost(app: App) {
  const host = app.server?.publicIp?.trim();
  if (host && host !== "127.0.0.1" && host !== "localhost" && host !== "0.0.0.0") return host;
  if (typeof window !== "undefined") return window.location.hostname;
  return host || null;
}
