"use client";

import { use, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { ArrowLeft, CheckCircle2, Clock, ScrollText, TerminalSquare, XCircle } from "lucide-react";
import { Nav } from "@/components/Nav";
import { api, apiUrl } from "@/lib/api";
import { PageHeader, StatusPill } from "@/components/ui";

type Deployment = {
  id: string;
  status: string;
  commitSha?: string | null;
  failure?: string | null;
};

type LogLine = {
  stream: string;
  line: string;
};

export default function DeploymentDetail({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const [deployment, setDeployment] = useState<Deployment | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [redirecting, setRedirecting] = useState(false);

  useEffect(() => {
    const loadDeployment = () => api<Deployment>(`/api/deployments/${id}`).then(setDeployment).catch(() => setDeployment({ id, status: "unknown", failure: "Deployment could not be loaded." }));
    loadDeployment();
    const poll = setInterval(loadDeployment, 2500);
    api<LogLine[]>(`/api/deployments/${id}/logs`).then((rows) => setLogs(rows.map((row) => `${row.stream}: ${row.line}`))).catch(() => {});
    const ws = new WebSocket(`${apiUrl().replace("http", "ws")}/ws/logs/${id}`);
    if (redirecting) {
      clearInterval(poll);
      ws.close();
      return () => {};
    }
    ws.onmessage = (event) => {
      const row = JSON.parse(event.data);
      setLogs((current) => [...current, `${row.stream}: ${row.line}`].slice(-1000));
    };
    return () => {
      clearInterval(poll);
      ws.close();
    };
  }, [id, redirecting]);

  useEffect(() => {
    if (deployment?.status !== "success" || redirecting) return;
    setRedirecting(true);
    const timer = setTimeout(() => window.location.assign("/apps"), 1600);
    return () => clearTimeout(timer);
  }, [deployment?.status, redirecting]);

  const steps = ["queued", "running", "building", "starting", "health_checking", "routing", "success"];
  const status = deployment?.status || "loading";
  const activeIndex = steps.indexOf(status);
  const groupedLogs = useMemo(() => logs.join("\n"), [logs]);

  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
          <PageHeader
            eyebrow="Deployment"
            title="Deployment logs"
            description={deployment?.commitSha ? `Commit ${deployment.commitSha}` : "Build, runtime, health check, and routing output."}
            actions={<Link className="button-secondary" href="/apps"><ArrowLeft size={16} />Back to apps</Link>}
          />

          <div className="grid gap-6 xl:grid-cols-[360px_minmax(0,1fr)]">
            <aside className="space-y-6">
              <section className="panel p-4">
                <div className="mb-4 flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <ScrollText size={18} />
                    <h2 className="font-semibold">Status</h2>
                  </div>
                  <StatusPill status={status} />
                </div>
                <div className="space-y-3">
                  {steps.map((step, index) => {
                    const done = status === "failed" ? index < Math.max(activeIndex, 0) : activeIndex >= index;
                    const current = status === step;
                    return (
                      <div key={step} className="flex items-center gap-3 rounded-md bg-panel px-3 py-2">
                        <span className={`flex h-7 w-7 items-center justify-center rounded-full ${status === "failed" && current ? "bg-red-100 text-red-700" : done ? "bg-emerald-100 text-emerald-700" : "bg-white text-neutral-500 ring-1 ring-line"}`}>
                          {status === "failed" && current ? <XCircle size={15} /> : done ? <CheckCircle2 size={15} /> : <Clock size={15} />}
                        </span>
                        <span className="text-sm font-medium">{humanStatus(step)}</span>
                      </div>
                    );
                  })}
                </div>
                <p className="muted mt-4">{statusHelp(status)}</p>
              </section>

              {redirecting && <div className="rounded-lg border border-emerald-200 bg-emerald-50 p-4 text-sm text-emerald-900">Deployment succeeded. Returning to Apps...</div>}
              {deployment?.failure && <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-900">{deployment.failure}</div>}
            </aside>

            <section className="min-w-0">
              <div className="mb-3 flex items-center justify-between gap-3">
                <div className="flex items-center gap-2">
                  <TerminalSquare size={18} />
                  <h2 className="font-semibold">Live output</h2>
                </div>
                <div className="text-xs text-neutral-500">{logs.length} lines</div>
              </div>
              <pre className="h-[68vh] max-w-full overflow-auto rounded-lg border border-neutral-800 bg-neutral-950 p-4 text-sm leading-6 text-green-100 shadow-sm shadow-neutral-950/20 [overflow-wrap:normal] [white-space:pre]">
                {groupedLogs || "Waiting for deployment logs..."}
              </pre>
            </section>
          </div>
        </div>
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
