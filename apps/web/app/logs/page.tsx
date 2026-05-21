"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";

type App = {
  id: string;
  name: string;
  repoFullName: string;
  latestDeployment?: { id: string; status?: string | null; finishedAt?: string | null; startedAt?: string | null } | null;
};

export default function Logs() {
  const [apps, setApps] = useState<App[]>([]);
  const [message, setMessage] = useState("Loading deployments...");

  useEffect(() => {
    api<App[]>("/api/apps")
      .then((rows) => {
        const withDeploys = rows.filter((app) => app.latestDeployment?.id);
        setApps(withDeploys);
        setMessage(withDeploys.length ? "" : "No deployment logs yet. Deploy an app to see logs here.");
      })
      .catch((e) => setMessage(`Could not load logs. ${e instanceof Error ? e.message : "Sign in again."}`));
  }, []);

  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <div className="mb-6 flex items-center justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold">Logs</h1>
            <p className="muted mt-1">Open the latest deployment logs for each app.</p>
          </div>
          <Link className="button" href="/apps/new">Create app</Link>
        </div>
        <div className="grid gap-3">
          {apps.map((app) => (
            <Link key={app.id} href={`/deployments/${app.latestDeployment?.id}`} className="rounded-lg border border-line bg-white p-4 hover:border-action">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="font-medium">{app.name}</div>
                  <p className="muted">{app.repoFullName}</p>
                </div>
                <span className="rounded-full bg-neutral-100 px-2 py-1 text-xs text-neutral-700">{app.latestDeployment?.status || "unknown"}</span>
              </div>
            </Link>
          ))}
          {message && (
            <div className="rounded-lg border border-line bg-white p-6">
              <p className="text-sm text-neutral-700">{message}</p>
              <Link className="button mt-4" href="/apps">View apps</Link>
            </div>
          )}
        </div>
      </section>
    </main>
  );
}
