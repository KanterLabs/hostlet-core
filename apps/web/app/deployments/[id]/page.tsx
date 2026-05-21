"use client";

import { use, useEffect, useState } from "react";
import { Nav } from "@/components/Nav";
import { api, apiUrl } from "@/lib/api";

export default function DeploymentDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const [deployment, setDeployment] = useState<any>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [redirecting, setRedirecting] = useState(false);
  useEffect(() => {
    const loadDeployment = () => api(`/api/deployments/${id}`).then(setDeployment).catch(() => setDeployment({ status: "unknown", failure: "Deployment could not be loaded." }));
    loadDeployment();
    const poll = setInterval(loadDeployment, 2500);
    api<any[]>(`/api/deployments/${id}/logs`).then((rows) => setLogs(rows.map((r) => `${r.stream}: ${r.line}`))).catch(() => {});
    const ws = new WebSocket(`${apiUrl().replace("http", "ws")}/ws/logs/${id}`);
    if (redirecting) {
      clearInterval(poll);
      ws.close();
      return () => {};
    }
    ws.onmessage = (e) => {
      const row = JSON.parse(e.data);
      setLogs((l) => [...l, `${row.stream}: ${row.line}`].slice(-1000));
    };
    return () => {
      clearInterval(poll);
      ws.close();
    };
  }, [id, redirecting]);

  useEffect(() => {
    if (deployment?.status !== "success" || redirecting) return;
    setRedirecting(true);
    const timer = setTimeout(() => window.location.assign("/apps"), 1200);
    return () => clearTimeout(timer);
  }, [deployment?.status, redirecting]);

  const steps = ["queued", "running", "building", "starting", "health_checking", "routing", "success"];
  const status = deployment?.status || "loading";
  const activeIndex = steps.indexOf(status);
  return (
    <main className="grid min-h-screen grid-cols-[220px_minmax(0,1fr)] overflow-x-hidden">
      <Nav />
      <section className="min-w-0 p-8">
        <h1 className="text-2xl font-semibold">Deployment</h1>
        <div className="mt-4 rounded-lg border border-line bg-white p-4">
          <div className="font-medium">Status: {humanStatus(status)}</div>
          <div className="mt-4 grid gap-2 md:grid-cols-7">
            {steps.map((step, index) => (
              <div key={step} className={`h-2 rounded-full ${status === "failed" ? "bg-red-200" : activeIndex >= index ? "bg-action" : "bg-neutral-200"}`} title={humanStatus(step)} />
            ))}
          </div>
          <p className="muted mt-3">{statusHelp(status)}</p>
          {redirecting && <p className="mt-3 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-sm text-emerald-900">Deployment succeeded. Returning to Apps...</p>}
          {deployment?.failure && <p className="mt-2 break-words rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800">What failed: {deployment.failure}</p>}
        </div>
        <pre className="mt-4 h-[60vh] max-w-full overflow-auto rounded-lg bg-ink p-4 text-sm text-green-100 [overflow-wrap:normal] [white-space:pre]">{logs.join("\n") || "Waiting for deployment logs..."}</pre>
      </section>
    </main>
  );
}

function humanStatus(status: string) {
  return status.replaceAll("_", " ");
}

function statusHelp(status: string) {
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
