"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Plus, ScrollText } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, EmptyState, PageHeader, Panel, StatusPill } from "@/components/ui";

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
        setMessage(withDeploys.length ? "" : "No deployment logs yet.");
      })
      .catch((error) => setMessage(`Could not load logs. ${error instanceof Error ? error.message : "Sign in again."}`));
  }, []);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Deployments"
            title="Logs"
            description="Jump into the latest deployment output for each app."
            actions={<Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>}
          />

          {apps.length > 0 ? (
            <div className="grid gap-4">
              {apps.map((app) => (
                <Panel key={app.id} className="transition hover:border-action">
                  <Link href={`/deployments/${app.latestDeployment?.id}`} className="block">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <ScrollText size={18} />
                        <div className="truncate text-lg font-semibold">{app.name}</div>
                      </div>
                      <p className="muted mt-1 truncate">{app.repoFullName}</p>
                    </div>
                    <div className="flex flex-wrap items-center gap-3">
                      <StatusPill status={app.latestDeployment?.status || "unknown"} />
                      <span className="text-sm text-muted">{formatTime(app.latestDeployment?.finishedAt || app.latestDeployment?.startedAt)}</span>
                    </div>
                  </div>
                  </Link>
                </Panel>
              ))}
            </div>
          ) : (
            <EmptyState
              icon={ScrollText}
              title={message}
              description="Deploy an app to generate build, health check, routing, and runtime logs."
              actionHref="/apps"
              actionLabel="View apps"
            />
          )}
    </AppShell>
  );
}

function formatTime(value?: string | null) {
  if (!value) return "No timestamp";
  return new Date(value).toLocaleString();
}
