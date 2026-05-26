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

type SessionPayload = {
  mode: "self_hosted" | "cloud";
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
  } | null;
};

export default function Logs() {
  const [apps, setApps] = useState<App[]>([]);
  const [session, setSession] = useState<SessionPayload | null>(null);
  const [message, setMessage] = useState("Loading deployments...");

  useEffect(() => {
    api<App[]>("/api/apps")
      .then((rows) => {
        const withDeploys = rows.filter((app) => app.latestDeployment?.id);
        setApps(withDeploys);
        setMessage(withDeploys.length ? "" : "No deployment logs yet.");
      })
      .catch((error) => setMessage(`Could not load logs. ${error instanceof Error ? error.message : "Sign in again."}`));
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
  }, []);

  const cloud = session?.mode === "cloud";
  const createDisabledReason = cloudCreateDisabledReason(session);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Deployments"
            title="Logs"
            description={cloud ? "Review the latest Hostlet Cloud deployment output for each app." : "Jump into the latest deployment output for each self-hosted app."}
            actions={createDisabledReason ? <button className="button" disabled title={createDisabledReason}><Plus size={16} />Create app</button> : <Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>}
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
              description={cloud ? "Deploy a cloud app to generate build, health check, routing, and runtime logs." : "Deploy an app on this machine to generate build, health check, routing, and runtime logs."}
              actionHref="/apps"
              actionLabel="View apps"
            />
          )}
    </AppShell>
  );
}

function cloudCreateDisabledReason(session: SessionPayload | null) {
  if (session?.mode !== "cloud") return "";
  if (!session.cloud?.githubInstalled) return "Install the Hostlet GitHub App before creating cloud apps.";
  if (!session.cloud.billingActive) return "Start a Stripe sandbox subscription before creating cloud apps.";
  if (session.cloud.nextStep !== "ready") return "Finish Hostlet Cloud setup before creating apps.";
  return "";
}

function formatTime(value?: string | null) {
  if (!value) return "No timestamp";
  return new Date(value).toLocaleString();
}
