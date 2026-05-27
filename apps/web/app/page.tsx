"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Box, CreditCard, GitBranch, HardDrive, Plus, Rocket, ShieldCheck } from "lucide-react";
import { GitHubStatus } from "@/components/GitHubStatus";
import { api } from "@/lib/api";
import { AppShell, DataList, DataRow, IconFrame, Metric, MetricsGrid, Notice, PageHeader, Panel, PanelHeader, SectionHeader, StatusPill } from "@/components/ui";
import { CloudUsagePanel, type CloudUsage } from "@/components/CloudUsagePanel";

type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  server?: { name: string; status: string; kind: string } | null;
  latestDeployment?: { id: string; status?: string | null; failure?: string | null; finishedAt?: string | null; startedAt?: string | null } | null;
  health?: { status: string; lastCheckedAt?: string | null } | null;
};

type Server = { id: string; name: string; kind: string; status: string; lastSeenAt?: string | null };
type VersionPayload = { currentVersion: string };
type SessionPayload = {
  mode: "self_hosted" | "cloud";
  authenticated: boolean;
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
    planCode?: string | null;
    subscriptionStatus?: string | null;
  } | null;
};

export default function Dashboard() {
  const [apps, setApps] = useState<App[]>([]);
  const [servers, setServers] = useState<Server[]>([]);
  const [version, setVersion] = useState<VersionPayload | null>(null);
  const [session, setSession] = useState<SessionPayload | null>(null);
  const [usage, setUsage] = useState<CloudUsage | null>(null);
  const [message, setMessage] = useState("Loading Hostlet...");
  const [billingMessage, setBillingMessage] = useState("");
  const [billingBusy, setBillingBusy] = useState<"student" | "starter" | "pro" | "">("");

  useEffect(() => {
    let active = true;
    async function loadDashboard() {
      try {
        const [appRows, serverRows] = await Promise.all([
          api<App[]>("/api/apps"),
          api<Server[]>("/api/servers"),
        ]);
        if (!active) return;
        setApps(appRows);
        setServers(serverRows);
        setMessage("");
      } catch (err) {
        if (!active) return;
        setMessage(err instanceof Error ? err.message : "Could not load Hostlet.");
      }
    }
    loadDashboard();
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") loadDashboard();
    }, 10000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    api<VersionPayload>("/api/system/version").then(setVersion).catch(() => setVersion(null));
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
    api<CloudUsage>("/api/cloud/usage").then(setUsage).catch(() => setUsage(null));
  }, []);

  const activeDeploys = apps.filter((app) => isActive(app.latestDeployment?.status)).length;
  const healthyApps = apps.filter((app) => app.health?.status === "healthy").length;
  const unhealthyApps = apps.filter((app) => app.health?.status === "unhealthy").length;
  const publicApps = apps.filter((app) => app.publicExposure).length;
  const onlineServers = servers.filter((server) => server.status === "online").length;
  const recentApps = useMemo(() => apps.slice(0, 5), [apps]);
  const cloud = session?.mode === "cloud";
  const cloudReady = session?.cloud?.nextStep === "ready";
  const createDisabledReason = cloudCreateDisabledReason(session, usage);

  async function startCheckout(plan: "student" | "starter" | "pro") {
    setBillingBusy(plan);
    setBillingMessage(`Opening ${planLabel(plan)} checkout...`);
    try {
      const result = await api<{ url?: string | null }>("/api/cloud/billing/checkout", {
        method: "POST",
        body: JSON.stringify({ plan }),
      });
      if (!result.url) throw new Error("Stripe did not return a checkout URL.");
      window.location.assign(result.url);
    } catch (error) {
      setBillingMessage(error instanceof Error ? error.message : "Checkout could not be opened.");
      setBillingBusy("");
    }
  }

  return (
    <AppShell>
          <PageHeader
            eyebrow={cloud ? "Hostlet Cloud" : "Control plane"}
            title="Overview"
            description={cloud ? "Deploy GitHub projects to always-on Hostlet Cloud URLs." : "Deploy GitHub projects onto your own machines with Docker, Caddy, live logs, rollbacks, and optional Cloudflare exposure."}
            actions={
              createDisabledReason ? (
                <button className="button" disabled title={createDisabledReason}><Plus size={16} />Create app</button>
              ) : (
                <Link className="button" href="/apps/new"><Plus size={16} />Create app</Link>
              )
            }
          />

          {cloud && session?.cloud && session.cloud.nextStep !== "ready" && (
            <Panel className="mb-6" padded>
              <SectionHeader icon={ShieldCheck} title="Finish Hostlet Cloud setup" description="Cloud deploys require GitHub App access and an active subscription before compute is available." />
              <div className="grid gap-3 md:grid-cols-3">
                <Metric label="GitHub login" value="Connected" detail="OAuth session active" icon={GitBranch} />
                <Metric label="GitHub App" value={session.cloud.githubInstalled ? "Installed" : "Required"} detail="Repo access for builds" icon={GitBranch} />
                <Metric label="Billing" value={session.cloud.billingActive ? "Active" : "Required"} detail="Required before deploy" icon={CreditCard} />
              </div>
              <div className="mt-4 flex flex-wrap gap-2">
                {!session.cloud.githubInstalled && <a className="button" href="/auth/github/install/start"><GitBranch size={16} />Install GitHub App</a>}
                {session.cloud.githubInstalled && !session.cloud.billingActive && (
                  <>
                    <button className="button" onClick={() => startCheckout("starter")} disabled={!!billingBusy}><CreditCard size={16} />{billingBusy === "starter" ? "Opening..." : "Start Starter"}</button>
                    <button className="button-secondary" onClick={() => startCheckout("student")} disabled={!!billingBusy}>{billingBusy === "student" ? "Opening..." : "Student"}</button>
                    <button className="button-secondary" onClick={() => startCheckout("pro")} disabled={!!billingBusy}>{billingBusy === "pro" ? "Opening..." : "Pro"}</button>
                  </>
                )}
              </div>
              {billingMessage && (
                <Notice
                  tone={billingMessage.toLowerCase().includes("could not") || billingMessage.toLowerCase().includes("did not") || billingMessage.toLowerCase().includes("failed") ? "danger" : "neutral"}
                  className="mt-3"
                  description={billingMessage}
                />
              )}
            </Panel>
          )}

          <MetricsGrid>
            <Metric label="Apps" value={String(apps.length)} detail={cloud && usage ? `${usage.apps.remaining} slots remaining` : `${healthyApps} healthy`} icon={Box} />
            <Metric label="Active deploys" value={String(activeDeploys)} detail="builds, checks, routing" icon={Rocket} />
            <Metric label="Unhealthy apps" value={String(unhealthyApps)} detail="runtime monitor" icon={ShieldCheck} />
            <Metric label="Public apps" value={String(publicApps)} detail={cloud ? "Hostlet Cloud URLs" : "Cloudflare DNS open"} icon={ShieldCheck} />
            <Metric label={cloud ? "Cloud worker" : "Machines online"} value={cloud ? "managed" : `${onlineServers}/${servers.length || 1}`} detail={cloud ? "Hostlet compute" : "agent heartbeat"} icon={HardDrive} />
          </MetricsGrid>

          <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_380px]">
            <Panel className="overflow-hidden" padded={false}>
              <PanelHeader title="Recent apps" description="Latest deployment state by project." action={<Link className="button-secondary" href="/apps">View all</Link>} />
              {recentApps.length > 0 ? (
                <div>
                  {recentApps.map((app) => (
                    <Link key={app.id} href={`/apps/${app.id}`} className="grid gap-3 border-t border-line px-4 py-4 first:border-t-0 hover:bg-surface-alt md:grid-cols-[1fr_170px_150px]">
                      <div className="min-w-0">
                        <div className="flex items-center gap-2">
                          <div className="truncate font-medium">{app.name}</div>
                          <StatusPill status={app.latestDeployment?.status || "not deployed"} />
                          <StatusPill status={app.health?.status || "unknown"} label={`health ${app.health?.status || "unknown"}`} />
                        </div>
                        <p className="muted mt-1 truncate">{app.repoFullName} · {app.branch}</p>
                      </div>
                      <div className="text-sm">
                        <div className="eyebrow">{cloud ? "Worker" : "Machine"}</div>
                        <div className="mt-1 truncate">{cloud ? "Hostlet Cloud" : app.server?.name || "Unknown"}</div>
                      </div>
                      <div className="text-sm">
                        <div className="eyebrow">Exposure</div>
                        <div className="mt-1">{app.publicExposure ? "Public" : "Private"}</div>
                      </div>
                    </Link>
                  ))}
                </div>
              ) : (
                <div className="p-6">
                  <div className="flex flex-col items-start">
                    <IconFrame icon={Box} className="mb-4" />
                    <div className="font-medium">No apps yet</div>
                    <p className="muted mt-2 max-w-xl">Create the first app, connect a GitHub repo, then start a deployment.</p>
                    {createDisabledReason ? (
                      <button className="mt-5" disabled title={createDisabledReason}>Create app</button>
                    ) : (
                      <Link className="button mt-5" href="/apps/new">Create app</Link>
                    )}
                  </div>
                </div>
              )}
            </Panel>

            <aside className="space-y-6">
              {cloud && <CloudUsagePanel usage={usage} />}
              <GitHubStatus />
              <Panel>
                <SectionHeader icon={GitBranch} title="Release state" />
                <DataList className="mt-4">
                  <DataRow label="Version" value={version?.currentVersion || "loading"} />
                  <DataRow label="Runtime" value={cloud ? "Hostlet Cloud worker" : "Docker + Caddy"} />
                  <DataRow label="Default access" value={cloud ? "Hostlet Cloud URL" : "Private apps"} />
                  <DataRow label="CI target" value="self-hosted Linux X64" />
                </DataList>
              </Panel>
            </aside>
          </div>

          {message && (
            <Notice tone="warning" className="mt-6" description={message} />
          )}
    </AppShell>
  );
}

function isActive(status?: string | null) {
  return !!status && ["queued", "running", "building", "starting", "health_checking", "routing"].includes(status);
}

function cloudCreateDisabledReason(session: SessionPayload | null, usage?: CloudUsage | null) {
  if (session?.mode !== "cloud") return "";
  if (!session.cloud?.githubInstalled) return "Install the Hostlet GitHub App before creating cloud apps.";
  if (!session.cloud.billingActive) return "Choose a Hostlet Cloud plan before creating apps.";
  if (session.cloud.nextStep !== "ready") return "Finish Hostlet Cloud setup before creating apps.";
  if (usage && usage.apps.limit > 0 && usage.apps.remaining <= 0) return "Your plan app limit is reached. Upgrade before creating another app.";
  return "";
}

function planLabel(plan: "student" | "starter" | "pro") {
  return plan === "student" ? "Student" : plan === "starter" ? "Starter" : "Pro";
}
