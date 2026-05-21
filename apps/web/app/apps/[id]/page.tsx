"use client";

import { use, useEffect, useState } from "react";
import Link from "next/link";
import { Nav } from "@/components/Nav";
import { api, apiUrl } from "@/lib/api";

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
  latestDeployment?: { id: string; status?: string | null; failure?: string | null } | null;
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
      } catch (e) {
        if (!active) return;
        setResources(null);
        setResourceMessage(e instanceof Error ? e.message : "Resource usage is not available yet.");
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
    } catch (e) {
      setMessage(`Deploy failed to start. ${e instanceof Error ? e.message : ""}`);
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
    } catch (e) {
      setMessage(`Rollback could not start. ${e instanceof Error ? e.message : ""}`);
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
    } catch (e) {
      setMessage(`Delete failed. ${e instanceof Error ? e.message : ""}`);
      setBusyAction("");
    }
  }

  async function toggleExposure() {
    if (!app || busyAction) return;
    const next = !app.publicExposure;
    setBusyAction("exposure");
    setMessage(next ? "Opening tunnel..." : "Closing tunnel...");
    try {
      await api(`/api/apps/${id}`, { method: "PATCH", body: JSON.stringify({ public_exposure: next }) });
      await refreshApp();
      setMessage(next ? "Tunnel opened. DNS may take a moment to propagate." : "Tunnel closed.");
    } catch (e) {
      setMessage(`${next ? "Open" : "Close"} tunnel failed. ${e instanceof Error ? e.message : ""}`);
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
      setMessage("Settings saved. Redeploy for runtime changes to take effect.");
    } catch (e) {
      setMessage(`Save failed. ${e instanceof Error ? e.message : ""}`);
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
    } catch (e) {
      setMessage(`Env save failed. ${e instanceof Error ? e.message : ""}`);
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
    } catch (e) {
      setMessage(`Env delete failed. ${e instanceof Error ? e.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }

  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold">{app?.name || "App"}</h1>
            {app && <p className="muted mt-2">{app.repoFullName} · {displayDomain(app.domain)}</p>}
          </div>
          <div className="flex flex-wrap gap-3">
            {app?.latestDeployment?.id && <Link className="button bg-white text-neutral-900 ring-1 ring-line hover:bg-panel" href={`/deployments/${app.latestDeployment.id}`}>View logs</Link>}
            {app && (
              <button disabled={!!busyAction} className="bg-white text-neutral-900 ring-1 ring-line hover:bg-panel" onClick={toggleExposure}>
                {busyAction === "exposure" ? "Updating..." : app.publicExposure ? "Close tunnel" : "Open tunnel"}
              </button>
            )}
            <button disabled={!!busyAction || isActiveDeploy(app?.latestDeployment?.status)} onClick={deploy}>{busyAction === "deploy" ? "Starting..." : "Deploy"}</button>
            <button disabled={!!busyAction || isActiveDeploy(app?.latestDeployment?.status)} onClick={rollback}>{busyAction === "rollback" ? "Starting..." : "Rollback"}</button>
            <button disabled={!!busyAction} className="bg-red-700 hover:bg-red-800" onClick={deleteApp}>{busyAction === "delete" ? "Deleting..." : "Delete"}</button>
          </div>
        </div>

        {app && !app.currentDeploymentId && (
          <div className="mt-6 rounded-lg border border-amber-200 bg-amber-50 p-4">
            <div className="font-medium text-amber-950">This app has not been deployed yet.</div>
            <p className="mt-1 text-sm text-amber-900">Start the first deployment to build, run, check health, and publish the route.</p>
            <button disabled={!!busyAction || isActiveDeploy(app?.latestDeployment?.status)} onClick={deploy} className="mt-4">{busyAction === "deploy" ? "Starting..." : "Start first deployment"}</button>
          </div>
        )}

        {app?.latestDeployment?.status === "failed" && (
          <div className="mt-6 rounded-lg border border-red-200 bg-red-50 p-4">
            <div className="font-medium text-red-950">Latest deployment failed.</div>
            <p className="mt-1 text-sm text-red-900">{app.latestDeployment.failure || "Open the logs to inspect the failure."}</p>
            {app.latestDeployment.id && <Link className="button mt-4 bg-white text-red-900 ring-1 ring-red-200 hover:bg-red-100" href={`/deployments/${app.latestDeployment.id}`}>View failure logs</Link>}
          </div>
        )}

        <section className="mt-8">
          <div className="mb-3 flex items-end justify-between gap-3">
            <div>
              <h2 className="text-lg font-semibold">Resource usage</h2>
              <p className="muted">Live Docker stats for the current running container.</p>
            </div>
            {resources?.sampledAt && <p className="text-xs text-neutral-500">Updated {new Date(resources.sampledAt).toLocaleTimeString()}</p>}
          </div>
          {resources ? (
            <div className="grid gap-3 md:grid-cols-3">
              <Metric label="CPU" value={resources.cpuPercent} />
              <Metric label="Memory" value={resources.memoryUsage} detail={resources.memoryPercent} />
              <Metric label="Processes" value={resources.pids} />
              <Metric label="Network I/O" value={resources.networkIo} />
              <Metric label="Disk I/O" value={resources.blockIo} />
              <Metric label="Container" value={app?.currentDeploymentId ? "running" : "not deployed"} />
            </div>
          ) : (
            <div className="rounded-lg border border-line bg-panel p-4 text-sm text-neutral-700">{resourceMessage}</div>
          )}
        </section>

        <section className="mt-8 grid gap-6 lg:grid-cols-[1fr_360px]">
          <div className="rounded-lg border border-line bg-white p-4">
            <h2 className="text-lg font-semibold">App settings</h2>
            <div className="mt-4 grid gap-4 md:grid-cols-2">
              <Field label="Domain" value={settings.domain} onChange={(value) => setSettings({ ...settings, domain: value })} />
              <Field label="Health path" value={settings.health_path} onChange={(value) => setSettings({ ...settings, health_path: value })} />
              <Field label="Root directory" value={settings.root_directory} onChange={(value) => setSettings({ ...settings, root_directory: value })} />
              <Field label="Container port" type="number" value={settings.container_port} onChange={(value) => setSettings({ ...settings, container_port: value })} />
              <Field label="Install command" value={settings.install_command} onChange={(value) => setSettings({ ...settings, install_command: value })} />
              <Field label="Build command" value={settings.build_command} onChange={(value) => setSettings({ ...settings, build_command: value })} />
              <Field label="Start command" value={settings.start_command} onChange={(value) => setSettings({ ...settings, start_command: value })} />
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="text-sm font-medium">Memory
                  <select value={settings.memory_limit_mb} onChange={(e) => setSettings({ ...settings, memory_limit_mb: e.target.value })}>
                    <option value="">No cap</option>
                    <option value="256">256 MB</option>
                    <option value="512">512 MB</option>
                    <option value="1024">1 GB</option>
                    <option value="2048">2 GB</option>
                    <option value="4096">4 GB</option>
                  </select>
                </label>
                <label className="text-sm font-medium">CPU
                  <select value={settings.cpu_limit} onChange={(e) => setSettings({ ...settings, cpu_limit: e.target.value })}>
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
              <label className="flex items-center gap-2 text-sm font-medium">
                <input type="checkbox" className="h-4 w-4" checked={settings.public_exposure} onChange={(e) => setSettings({ ...settings, public_exposure: e.target.checked })} />
                Public tunnel
              </label>
              <label className="flex items-center gap-2 text-sm font-medium">
                <input type="checkbox" className="h-4 w-4" checked={settings.auto_deploy} onChange={(e) => setSettings({ ...settings, auto_deploy: e.target.checked })} />
                Auto redeploy on branch push
              </label>
            </div>
            <button className="mt-4" disabled={!!busyAction} onClick={saveSettings}>{busyAction === "settings" ? "Saving..." : "Save settings"}</button>
          </div>

          <div className="rounded-lg border border-line bg-white p-4">
            <h2 className="text-lg font-semibold">Environment</h2>
            <div className="mt-4 space-y-3">
              {envKeys.map(({ key }) => (
                <div key={key} className="rounded-md border border-line p-3">
                  <div className="mb-2 flex items-center justify-between gap-3">
                    <span className="font-mono text-sm">{key}</span>
                    <button className="bg-white text-red-700 ring-1 ring-red-200 hover:bg-red-50" disabled={!!busyAction} onClick={() => deleteEnvVar(key)}>Delete</button>
                  </div>
                  <div className="flex gap-2">
                    <input type="password" value={envValues[key] || ""} onChange={(e) => setEnvValues({ ...envValues, [key]: e.target.value })} placeholder="New value" />
                    <button disabled={!!busyAction || !envValues[key]} onClick={() => saveEnvVar(key, envValues[key])}>Save</button>
                  </div>
                </div>
              ))}
              {envKeys.length === 0 && <p className="muted">No environment variables set.</p>}
              <div className="rounded-md border border-line bg-panel p-3">
                <input value={newEnv.key} onChange={(e) => setNewEnv({ ...newEnv, key: e.target.value.toUpperCase() })} placeholder="KEY" />
                <input className="mt-2" type="password" value={newEnv.value} onChange={(e) => setNewEnv({ ...newEnv, value: e.target.value })} placeholder="Value" />
                <button className="mt-2" disabled={!!busyAction || !newEnv.key || !newEnv.value} onClick={() => saveEnvVar(newEnv.key, newEnv.value)}>Add variable</button>
              </div>
            </div>
          </div>
        </section>

        <section className="mt-8 rounded-lg border border-line bg-white p-4">
          <h2 className="text-lg font-semibold">Automation</h2>
          <div className="mt-3 grid gap-3 md:grid-cols-3">
            <Metric label="Auto redeploy" value={app?.autoDeploy ? "enabled" : "disabled"} />
            <Metric label="Public tunnel" value={app?.publicExposure ? "open" : "closed"} />
            <Metric label="Latest webhook" value={webhookSummary(app?.latestWebhook)} />
          </div>
          <div className="mt-4 rounded-md border border-line bg-panel p-3 text-sm">
            <div className="font-medium">GitHub webhook</div>
            <div className="mt-2 break-all font-mono text-xs">{apiUrl()}/webhooks/github</div>
            <div className="muted mt-2">Use content type application/json, event push, and the configured GITHUB_WEBHOOK_SECRET.</div>
          </div>
        </section>

        {message && <p className="mt-4 rounded-md border border-line bg-panel p-3 text-sm">{message}</p>}
      </section>
    </main>
  );
}

function Field({ label, value, onChange, type = "text" }: { label: string; value: string; type?: string; onChange: (value: string) => void }) {
  return (
    <label className="text-sm font-medium">{label}
      <input type={type} value={value} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}

function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="rounded-lg border border-line bg-white p-4">
      <div className="text-xs font-medium uppercase tracking-wide text-neutral-500">{label}</div>
      <div className="mt-2 truncate text-xl font-semibold">{value}</div>
      {detail && <div className="muted mt-1">{detail}</div>}
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
