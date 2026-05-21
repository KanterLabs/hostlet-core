"use client";

import { use, useEffect, useState } from "react";
import Link from "next/link";
import {
  Activity,
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
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";
import { webhookReadiness } from "@/lib/webhooks";
import { Field, Metric, PageHeader, StatusPill } from "@/components/ui";
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

type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  containerPort?: number | null;
  healthPath?: string | null;
  rootDirectory?: string | null;
  installCommand?: string | null;
  buildCommand?: string | null;
  startCommand?: string | null;
  memoryLimitMb?: number | null;
  cpuLimit?: number | null;
  currentDeploymentId?: string | null;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  server?: { id: string; name: string; kind: string; status: string; lastSeenAt?: string | null } | null;
  latestDeployment?: { id: string; status?: string | null; failure?: string | null; commitSha?: string | null; startedAt?: string | null; finishedAt?: string | null } | null;
  latestWebhook?: {
    status: string;
    ignoredReason?: string | null;
    commitSha?: string | null;
    branch?: string | null;
    createdAt?: string | null;
  } | null;
};

type AgentJob = {
  id: string;
  status: "queued" | "running" | "success" | "failed";
  failure?: string | null;
};

type SettingsForm = {
  domain: string;
  health_path: string;
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

const emptySettings: SettingsForm = {
  domain: "",
  health_path: "/",
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
  const [app, setApp] = useState<App | null>(null);
  const [settings, setSettings] = useState<SettingsForm>(emptySettings);
  const [resources, setResources] = useState<ResourceStats | null>(null);
  const [envKeys, setEnvKeys] = useState<Array<{ key: string }>>([]);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [newEnv, setNewEnv] = useState({ key: "", value: "" });
  const [resourceMessage, setResourceMessage] = useState("Waiting for a successful deploy.");
  const [message, setMessage] = useState("");
  const [busyAction, setBusyAction] = useState<"deploy" | "rollback" | "exposure" | "delete" | "settings" | "env" | "">("");

  useEffect(() => {
    refreshApp();
    api<Array<{ key: string }>>(`/api/apps/${id}/env`).then(setEnvKeys).catch(() => setEnvKeys([]));
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
      setSettings({
        domain: loaded.domain || "",
        health_path: loaded.healthPath || "/",
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
      location.href = `/deployments/${res.deploymentId}`;
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
      location.href = `/deployments/${res.rollbackDeploymentId}`;
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
      location.href = "/apps";
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
      await api(`/api/apps/${id}`, {
        method: "PATCH",
        body: JSON.stringify({
          domain: settings.domain,
          health_path: settings.health_path,
          root_directory: settings.root_directory || ".",
          install_command: settings.install_command.trim() || null,
          build_command: settings.build_command.trim() || null,
          start_command: settings.start_command.trim() || null,
          container_port: Number(settings.container_port),
          memory_limit_mb: settings.memory_limit_mb ? Number(settings.memory_limit_mb) : null,
          cpu_limit: settings.cpu_limit ? Number(settings.cpu_limit) : null,
          public_exposure: settings.public_exposure,
          auto_deploy: settings.auto_deploy,
        }),
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

  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
          <PageHeader
            eyebrow="Application"
            title={app?.name || "App"}
            description={app ? `${app.repoFullName} · ${app.branch} · ${displayDomain(app.domain)}` : "Loading app..."}
            actions={
              <>
                {app?.latestDeployment?.id && <Link className="button-secondary" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />Logs</Link>}
                {app && (
                  <button disabled={!!busyAction} className="button-secondary" onClick={toggleExposure}>
                    <Globe2 size={16} />
                    {busyAction === "exposure" ? "Updating..." : app.publicExposure ? "Make private" : "Publish URL"}
                  </button>
                )}
                <button disabled={!!busyAction || active} onClick={deploy}><Play size={16} />{busyAction === "deploy" ? "Starting..." : "Deploy latest"}</button>
                <button disabled={!!busyAction || active} className="button-secondary" onClick={rollback}><RotateCcw size={16} />Rollback</button>
                <button disabled={!!busyAction} className="button-danger" onClick={deleteApp}><Trash2 size={16} />Delete</button>
              </>
            }
          />

          <div className="mb-6 grid gap-4 md:grid-cols-2 xl:grid-cols-4">
            <Metric label="Deployment" value={deploymentStatus.replaceAll("_", " ")} detail={shortSha(app?.latestDeployment?.commitSha)} icon={Activity} />
            <Metric label="Machine" value={app?.server?.name || "Unknown"} detail={app?.server?.status || "offline"} icon={Box} />
            <Metric label="Exposure" value={app?.publicExposure ? "public" : "private"} detail={displayDomain(app?.domain || "") || "No domain"} icon={Globe2} />
            <Metric label="Automation" value={app?.autoDeploy ? "auto deploy" : "manual"} detail={webhookSummary(app?.latestWebhook)} icon={GitBranch} />
          </div>

          <div className="mb-6 flex flex-wrap gap-2">
            <StatusPill status={deploymentStatus} />
            <StatusPill status={app?.server?.status || "offline"} label={`machine ${app?.server?.status || "offline"}`} />
            <StatusPill status={app?.publicExposure ? "open" : "closed"} label={app?.publicExposure ? "public URL" : "private app"} />
          </div>

          <WebhookNotice autoDeployEnabled={!!app?.autoDeploy} onManualDeploy={deploy} deployDisabled={!!busyAction || active} className="mb-6" />

          {app && !app.currentDeploymentId && (
            <div className="mb-6 rounded-lg border border-amber-200 bg-amber-50 p-4">
              <div className="font-medium text-amber-950">This app has not been deployed yet.</div>
              <p className="mt-1 text-sm text-amber-900">Start the first deployment to build, run, check health, and publish the route.</p>
              <button disabled={!!busyAction || active} onClick={deploy} className="mt-4"><Play size={16} />Start first deployment</button>
            </div>
          )}

          {app?.latestDeployment?.status === "failed" && (
            <div className="mb-6 rounded-lg border border-red-200 bg-red-50 p-4">
              <div className="font-medium text-red-950">Latest deployment failed.</div>
              <p className="mt-1 text-sm text-red-900">{app.latestDeployment.failure || "Open the logs to inspect the failure."}</p>
              {app.latestDeployment.id && <Link className="button-secondary mt-4 text-red-900 ring-red-200 hover:bg-red-100" href={`/deployments/${app.latestDeployment.id}`}><ScrollText size={16} />View failure logs</Link>}
            </div>
          )}

          <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_390px]">
            <div className="space-y-6">
              <section>
                <div className="mb-4 flex items-center justify-between gap-3">
                  <div>
                    <h2 className="font-semibold">Resource usage</h2>
                    <p className="muted mt-1">Live Docker stats for the current running container.</p>
                  </div>
                  {resources?.sampledAt && <p className="text-xs text-muted">Updated {new Date(resources.sampledAt).toLocaleTimeString()}</p>}
                </div>
                {resources ? (
                  <div className="grid gap-3 md:grid-cols-3">
                    <Metric label="CPU" value={cpu.value} detail={cpu.detail} icon={Cpu} />
                    <Metric label="Memory" value={resources.memoryUsage} detail={resources.memoryPercent} />
                    <Metric label="Processes" value={resources.pids} />
                    <Metric label="Network I/O" value={resources.networkIo} />
                    <Metric label="Disk I/O" value={resources.blockIo} />
                    <Metric label="Container" value={app?.currentDeploymentId ? "running" : "not deployed"} />
                  </div>
                ) : (
                  <div className="rounded-lg border border-line bg-surface-alt p-4 text-sm text-muted">{resourceMessage}</div>
                )}
              </section>

              <section className="panel p-4">
                <div className="mb-4 flex items-center gap-2">
                  <Settings size={18} />
                  <h2 className="font-semibold">App settings</h2>
                </div>
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="Domain" value={settings.domain} onChange={(value) => setSettings({ ...settings, domain: value })} />
                  <Field label="Health path" value={settings.health_path} onChange={(value) => setSettings({ ...settings, health_path: value })} />
                  <Field label="Root directory" value={settings.root_directory} onChange={(value) => setSettings({ ...settings, root_directory: value })} />
                  <Field label="Container port" type="number" value={settings.container_port} onChange={(value) => setSettings({ ...settings, container_port: value })} />
                  <Field label="Install command" value={settings.install_command} onChange={(value) => setSettings({ ...settings, install_command: value })} />
                  <Field label="Build command" value={settings.build_command} onChange={(value) => setSettings({ ...settings, build_command: value })} />
                  <Field label="Start command" value={settings.start_command} onChange={(value) => setSettings({ ...settings, start_command: value })} />
                  <div className="grid gap-3 sm:grid-cols-2">
                    <label className="block">Memory
                      <select className="mt-1" value={settings.memory_limit_mb} onChange={(event) => setSettings({ ...settings, memory_limit_mb: event.target.value })}>
                        <option value="">No cap</option>
                        <option value="256">256 MB</option>
                        <option value="512">512 MB</option>
                        <option value="1024">1 GB</option>
                        <option value="2048">2 GB</option>
                        <option value="4096">4 GB</option>
                      </select>
                    </label>
                    <label className="block">CPU
                      <select className="mt-1" value={settings.cpu_limit} onChange={(event) => setSettings({ ...settings, cpu_limit: event.target.value })}>
                        <option value="">No cap</option>
                        <option value="0.25">0.25 CPU</option>
                        <option value="0.5">0.5 CPU</option>
                        <option value="1">1 CPU</option>
                        <option value="2">2 CPUs</option>
                        <option value="4">4 CPUs</option>
                      </select>
                    </label>
                  </div>
                </div>
                <div className="mt-4 flex flex-wrap gap-4">
                  <label className="flex items-center gap-2 rounded-md border border-line bg-surface-alt px-3 py-2">
                    <input type="checkbox" checked={settings.public_exposure} onChange={(event) => setSettings({ ...settings, public_exposure: event.target.checked })} />
                    Public URL
                  </label>
                  <label className="flex items-center gap-2 rounded-md border border-line bg-surface-alt px-3 py-2">
                    <input type="checkbox" checked={settings.auto_deploy} onChange={(event) => setSettings({ ...settings, auto_deploy: event.target.checked })} />
                    Auto redeploy on branch push
                  </label>
                </div>
                <button className="mt-4" disabled={!!busyAction} onClick={saveSettings}><Save size={16} />{busyAction === "settings" ? "Saving..." : "Save settings"}</button>
              </section>
            </div>

            <aside className="space-y-6">
              <section className="panel p-4">
                <div className="mb-4 flex items-center gap-2">
                  <KeyRound size={18} />
                  <h2 className="font-semibold">Environment</h2>
                </div>
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
              </section>

              <section className="panel p-4">
                <h2 className="font-semibold">Automation</h2>
                <div className="mt-4 grid gap-2">
                  <Summary label="Auto redeploy" value={app?.autoDeploy ? "enabled" : "disabled"} />
                  <Summary label="Public URL" value={app?.publicExposure ? "published" : "private"} />
                  <Summary label="Latest webhook" value={webhookSummary(app?.latestWebhook)} />
                </div>
                <div className="mt-4 rounded-md border border-line bg-surface-alt p-3 text-sm">
                  <div className="font-medium">GitHub webhook</div>
                  <div className="mt-2 break-all font-mono text-xs">{webhook.webhookUrl}</div>
                </div>
              </section>

              {app?.publicExposure && app.domain && (
                <a className="button-secondary w-full" href={externalHref(app.domain)} target="_blank" rel="noreferrer">
                  <ExternalLink size={16} />
                  Open app URL
                </a>
              )}
            </aside>
          </div>

          {message && <p className="mt-6 rounded-md border border-line bg-surface p-3 text-sm shadow-sm shadow-neutral-950/5">{message}</p>}
        </div>
      </section>
    </main>
  );
}

function Summary({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md bg-surface-alt px-3 py-2 text-sm">
      <div className="eyebrow">{label}</div>
      <div className="mt-1 break-words font-medium">{value}</div>
    </div>
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

function externalHref(domain: string) {
  const display = displayDomain(domain);
  if (!display) return "#";
  if (display.startsWith("http://") || display.startsWith("https://")) return display;
  try {
    const url = new URL(`http://${display}`);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1" || /^[\d.]+$/.test(url.hostname)) {
      return `http://${display}`;
    }
  } catch {
    return "#";
  }
  return `https://${display}`;
}
