"use client";

import { use, useEffect, useState } from "react";
import Link from "next/link";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";

type ResourceStats = {
  cpuPercent: string;
  memoryUsage: string;
  memoryPercent: string;
  networkIo: string;
  blockIo: string;
  pids: string;
  sampledAt: string;
};

export default function AppDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const [app, setApp] = useState<any>(null);
  const [resources, setResources] = useState<ResourceStats | null>(null);
  const [resourceMessage, setResourceMessage] = useState("Waiting for a successful deploy.");
  const [message, setMessage] = useState("");
  const [busyAction, setBusyAction] = useState<"deploy" | "rollback" | "delete" | "">("");

  useEffect(() => {
    api(`/api/apps/${id}`).then(setApp).catch(() => setMessage("Could not load app. Sign in and check that it still exists."));
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

  async function deploy() {
    if (busyAction) return;
    setBusyAction("deploy");
    setMessage("Starting deployment...");
    try {
      const res = await api<{ deploymentId: string }>(`/api/apps/${id}/deploy`, { method: "POST", body: "{}" });
      location.href = `/deployments/${res.deploymentId}`;
    } catch (e) {
      setMessage(`Deploy failed to start. The server agent may be offline. ${e instanceof Error ? e.message : ""}`);
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
      setMessage(`Rollback could not start. A previous successful deployment may not exist. ${e instanceof Error ? e.message : ""}`);
      setBusyAction("");
    }
  }

  async function deleteApp() {
    if (!confirm("Delete this app and its deployment history? Running containers are not removed yet.")) return;
    if (busyAction) return;
    setBusyAction("delete");
    setMessage("Deleting app...");
    try {
      await api(`/api/apps/${id}`, { method: "DELETE" });
      location.href = "/apps";
    } catch (e) {
      setMessage(`Delete failed. ${e instanceof Error ? e.message : ""}`);
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
          <div className="flex gap-3">
            {app?.latestDeployment?.id && <Link className="button bg-white text-neutral-900 ring-1 ring-line hover:bg-panel" href={`/deployments/${app.latestDeployment.id}`}>View logs</Link>}
            <button disabled={!!busyAction} onClick={deploy}>{busyAction === "deploy" ? "Starting..." : "Deploy"}</button>
            <button disabled={!!busyAction} onClick={rollback}>{busyAction === "rollback" ? "Starting..." : "Rollback"}</button>
            <button disabled={!!busyAction} className="bg-red-700 hover:bg-red-800" onClick={deleteApp}>{busyAction === "delete" ? "Deleting..." : "Delete"}</button>
          </div>
        </div>

        {app && !app.currentDeploymentId && (
          <div className="mt-6 rounded-lg border border-amber-200 bg-amber-50 p-4">
            <div className="font-medium text-amber-950">This app has not been deployed yet.</div>
            <p className="mt-1 text-sm text-amber-900">
              Start the first deployment to build the repo, run the container, check health, and publish the local URL.
            </p>
            <button disabled={!!busyAction} onClick={deploy} className="mt-4">
              {busyAction === "deploy" ? "Starting..." : "Start first deployment"}
            </button>
          </div>
        )}

        {app?.latestDeployment?.status === "failed" && (
          <div className="mt-6 rounded-lg border border-red-200 bg-red-50 p-4">
            <div className="font-medium text-red-950">Latest deployment failed.</div>
            <p className="mt-1 text-sm text-red-900">Open the logs to see what failed, then deploy again after fixing the repo or settings.</p>
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
          {app && (
            <div className="mt-3 grid gap-3 md:grid-cols-2">
              <Metric label="Configured memory limit" value={app.memoryLimitMb ? `${app.memoryLimitMb} MB` : "No limit set"} />
              <Metric label="Configured CPU limit" value={app.cpuLimit ? `${app.cpuLimit} CPU` : "No limit set"} />
            </div>
          )}
        </section>

        {message && <p className="mt-4 rounded-md border border-line bg-panel p-3 text-sm">{message}</p>}
      </section>
    </main>
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
