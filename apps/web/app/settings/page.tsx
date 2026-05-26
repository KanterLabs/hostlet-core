"use client";

import { useEffect, useState } from "react";
import type { LucideIcon } from "lucide-react";
import { Cloud, CreditCard, Download, GitBranch, KeyRound, Link2, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, DataList, DataRow, IconFrame, Notice, PageHeader, Panel, StatusPill } from "@/components/ui";
import { WebhookNotice } from "@/components/WebhookNotice";
import { GitHubDeviceFlow } from "@/components/GitHubDeviceFlow";

type StatusPayload = {
  configured?: boolean;
  oauthConfigured?: boolean;
  webhookConfigured?: boolean;
  tokenValid?: boolean | null;
  authenticated?: boolean;
  login?: string | null;
  baseDomain?: string | null;
  domainPrefix?: string;
  defaultDomainPattern?: string | null;
  tunnelTargetConfigured?: boolean;
  message: string;
};

type VersionPayload = {
  currentVersion: string;
  updateChecksEnabled: boolean;
  update?: UpdatePayload | null;
};

type UpdatePayload = {
  latestVersion?: string;
  releaseNotesUrl?: string;
  releasedAt?: string;
  minimumSupportedVersion?: string | null;
  composeMigrations?: boolean;
  databaseMigrations?: boolean;
  updateAvailable?: boolean;
  unsupportedDirectUpdate?: boolean;
  checkedAt?: string;
};

type AgentJob = {
  id: string;
  type: string;
  status: string;
  failure?: string | null;
  attempt: number;
  maxAttempts: number;
  createdAt: string;
  finishedAt?: string | null;
};

type AuditEvent = {
  id: string;
  eventType: string;
  actorType: string;
  appId?: string | null;
  jobId?: string | null;
  createdAt: string;
};

type CleanupPlan = {
  database: Record<string, number>;
  docker: { keepContainers: number; keepImages: number; jobWillRun: boolean };
};

type BackupMetadata = {
  created_at?: string;
  path?: string;
  scheduled?: string;
};

type SessionPayload = {
  mode: "self_hosted" | "cloud";
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
  } | null;
};

export default function Settings() {
  const [session, setSession] = useState<SessionPayload | null>(null);
  const [github, setGithub] = useState<StatusPayload | null>(null);
  const [cloudflare, setCloudflare] = useState<StatusPayload | null>(null);
  const [version, setVersion] = useState<VersionPayload | null>(null);
  const [jobs, setJobs] = useState<AgentJob[]>([]);
  const [audit, setAudit] = useState<AuditEvent[]>([]);
  const [cleanup, setCleanup] = useState<CleanupPlan | null>(null);
  const [backup, setBackup] = useState<BackupMetadata | null>(null);
  const [updateMessage, setUpdateMessage] = useState("");
  const [operationsMessage, setOperationsMessage] = useState("");

  useEffect(() => {
    refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") {
        api<VersionPayload>("/api/system/version").then(setVersion).catch(() => setVersion(null));
      }
    }, 30 * 60 * 1000);
    return () => window.clearInterval(timer);
  }, []);

  function refresh() {
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
    api<StatusPayload>("/api/github/status").then(setGithub).catch((error) => setGithub({ message: error instanceof Error ? error.message : "Could not load GitHub status." }));
    api<StatusPayload>("/api/cloudflare/status").then(setCloudflare).catch((error) => setCloudflare({ message: error instanceof Error ? error.message : "Could not load Cloudflare status." }));
    api<VersionPayload>("/api/system/version").then(setVersion).catch(() => setVersion(null));
    api<AgentJob[]>("/api/agent-jobs").then(setJobs).catch(() => setJobs([]));
    api<AuditEvent[]>("/api/audit-events").then(setAudit).catch(() => setAudit([]));
    api<CleanupPlan>("/api/system/cleanup").then(setCleanup).catch(() => setCleanup(null));
    api<BackupMetadata | undefined>("/api/system/backups/latest").then((value) => setBackup(value || null)).catch(() => setBackup(null));
  }

  async function checkForUpdates() {
    setUpdateMessage("Checking for updates...");
    try {
      const update = await api<UpdatePayload>("/api/system/update-check", { method: "POST", body: "{}" });
      setVersion((current) => current ? { ...current, update } : current);
      setUpdateMessage(update.updateAvailable ? "Update available. Run hostlet update on the server." : "Hostlet is up to date.");
    } catch (error) {
      setUpdateMessage(error instanceof Error ? error.message : "Could not check for updates.");
    }
  }

  async function runCleanup() {
    setOperationsMessage("Cleanup requested...");
    try {
      await api("/api/system/cleanup", { method: "POST", body: "{}" });
      setOperationsMessage("Cleanup started. Docker cleanup will appear as an agent job.");
      refresh();
    } catch (error) {
      setOperationsMessage(error instanceof Error ? error.message : "Cleanup failed.");
    }
  }

  async function retryJob(id: string) {
    await api(`/api/agent-jobs/${id}/retry`, { method: "POST", body: "{}" });
    refresh();
  }

  async function cancelJob(id: string) {
    await api(`/api/agent-jobs/${id}/cancel`, { method: "POST", body: "{}" });
    refresh();
  }

  async function startCheckout(plan: "student" | "starter" | "pro") {
    const result = await api<{ url?: string | null }>("/api/cloud/billing/checkout", {
      method: "POST",
      body: JSON.stringify({ plan }),
    });
    if (result.url) window.location.assign(result.url);
  }

  const cloud = session?.mode === "cloud";
  const cloudReady = session?.cloud?.nextStep === "ready";

  return (
    <AppShell>
          <PageHeader
            eyebrow={cloud ? "Hostlet Cloud" : "Control plane"}
            title="Settings"
            description={cloud ? "Cloud account, GitHub App, billing, and managed worker operations." : "Connection status for GitHub auth, webhooks, Cloudflare DNS, and public app routing."}
            actions={<button className="button-secondary" onClick={refresh}><RefreshCw size={16} />Refresh</button>}
          />

          {!cloud && <WebhookNotice className="mb-6" />}
          {cloud && session?.cloud && !cloudReady && (
            <Notice
              tone="warning"
              className="mb-6"
              title="Finish Hostlet Cloud setup"
              description="Cloud deploys require GitHub App access and an active Stripe sandbox subscription before compute is available."
              action={
                <>
                  {!session.cloud.githubInstalled && <a className="button" href="/auth/github/install/start"><GitBranch size={16} />Install GitHub App</a>}
                  {session.cloud.githubInstalled && !session.cloud.billingActive && <button onClick={() => startCheckout("starter")}><CreditCard size={16} />Start Starter</button>}
                </>
              }
            />
          )}

          <div className="grid gap-6 lg:grid-cols-2">
            <div className="space-y-3">
              <StatusCard
                icon={GitBranch}
                title={cloud ? "GitHub App" : "GitHub"}
                status={cloud ? session?.cloud?.githubInstalled ? "connected" : "needs attention" : github?.tokenValid ? "connected" : github?.oauthConfigured ? "needs attention" : "not configured"}
                message={cloud ? session?.cloud?.githubInstalled ? "Hostlet Cloud can access selected repositories." : "Install the Hostlet GitHub App before deploying cloud apps." : github?.message || "Loading GitHub status..."}
                rows={cloud ? [
                  ["Cloud auth", "GitHub OAuth redirect"],
                  ["Repository access", session?.cloud?.githubInstalled ? "GitHub App installed" : "GitHub App required"],
                  ["Account", github?.login || (github?.authenticated ? "signed in" : "not signed in")],
                ] : [
                  ["Device Flow", github?.oauthConfigured ? "configured" : "missing"],
                  ["Webhook secret", github?.webhookConfigured ? "configured" : "missing"],
                  ["Account", github?.login || (github?.authenticated ? "signed in" : "not signed in")],
                ]}
              />
              {!cloud && github?.oauthConfigured && github?.tokenValid !== true && <GitHubDeviceFlow buttonLabel="Reconnect GitHub" />}
              {cloud && !session?.cloud?.githubInstalled && <a className="button" href="/auth/github/install/start"><GitBranch size={16} />Install GitHub App</a>}
            </div>
            {cloud ? (
              <StatusCard
                icon={CreditCard}
                title="Billing"
                status={session?.cloud?.billingActive ? "connected" : "needs attention"}
                message={session?.cloud?.billingActive ? "Stripe sandbox subscription is active." : "Start a Stripe sandbox subscription before creating or mutating cloud apps."}
                rows={[
                  ["Billing mode", "Stripe sandbox"],
                  ["Subscription", session?.cloud?.billingActive ? "active" : "required"],
                  ["Compute gate", cloudReady ? "ready" : "blocked"],
                  ["App URLs", "*.hostlet.cloud"],
                ]}
              />
            ) : (
              <StatusCard
                icon={Cloud}
                title="Cloudflare"
                status={cloudflare?.tokenValid ? "connected" : cloudflare?.configured ? "needs attention" : "not configured"}
                message={cloudflare?.message || "Loading Cloudflare status..."}
                rows={[
                  ["Base domain", cloudflare?.baseDomain || "missing"],
                  ["App domains", cloudflare?.defaultDomainPattern || "missing"],
                  ["Legacy prefix", cloudflare?.domainPrefix || "hostlet-"],
                  ["Tunnel target", cloudflare?.tunnelTargetConfigured ? "configured" : "missing"],
                ]}
              />
            )}
          </div>

          {!cloud && <Panel className="mt-6">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex items-center gap-3">
                <IconFrame icon={Download} />
                <div>
                  <h2 className="font-semibold">Hostlet updates</h2>
                  <p className="muted mt-1">Detect new releases and run the owner-controlled CLI update flow.</p>
                </div>
              </div>
              <StatusPill status={version?.update?.updateAvailable ? "needs attention" : "connected"} label={version?.update?.updateAvailable ? "update available" : "current"} />
            </div>
            <DataList className="mt-5">
              <DataRow label="Current version" value={version?.currentVersion || "loading"} />
              <DataRow label="Latest version" value={version?.update?.latestVersion || "not checked"} />
              <DataRow label="Minimum supported" value={version?.update?.minimumSupportedVersion || "not specified"} />
              <DataRow label="Migrations" value={updateMigrationSummary(version?.update)} />
              <DataRow label="Last checked" value={version?.update?.checkedAt ? new Date(version.update.checkedAt).toLocaleString() : "not checked"} />
              <DataRow label="Update command" value="hostlet update" />
              <DataRow label="Latest backup" value={backup?.created_at ? `${formatBackupDate(backup.created_at)}${backup.scheduled === "true" ? " (scheduled)" : ""}` : "not recorded"} />
            </DataList>
            <div className="mt-4 flex flex-wrap gap-2">
              <button className="button-secondary" onClick={checkForUpdates} disabled={version?.updateChecksEnabled === false}>
                <RefreshCw size={16} />
                Check for updates
              </button>
              {version?.update?.releaseNotesUrl && (
                <a className="button-secondary" href={version.update.releaseNotesUrl} target="_blank" rel="noreferrer">
                  Release notes
                </a>
              )}
            </div>
            {updateMessage && <Notice tone={updateMessage.toLowerCase().includes("could not") ? "danger" : "neutral"} className="mt-3" description={updateMessage} />}
            {version?.update?.unsupportedDirectUpdate && <Notice tone="danger" className="mt-3" description="This release does not support a direct update from the installed version. Read the release notes before upgrading." />}
            {version?.updateChecksEnabled === false && <Notice tone="neutral" className="mt-3" description="Update checks are disabled by HOSTLET_UPDATE_CHECKS=false." />}
          </Panel>}

          <section className="mt-6 grid gap-4 md:grid-cols-3">
            <Info icon={ShieldCheck} title="Control-plane access" value={cloud ? "GitHub OAuth session" : "Password + GitHub Device Flow"} />
            <Info icon={KeyRound} title="Secrets" value="Encrypted app env vars" />
            <Info icon={Link2} title="App exposure" value={cloud ? "Managed *.hostlet.cloud URL" : "Private by default, public under base domain"} />
          </section>

          <Panel className="mt-6">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex items-center gap-3">
                <IconFrame icon={Trash2} />
                <div>
                  <h2 className="font-semibold">Cleanup and operations</h2>
                  <p className="muted mt-1">Preview retention counts, start cleanup, and recover recent durable jobs.</p>
                </div>
              </div>
              {!cloud && <button className="button-secondary" onClick={runCleanup}><Trash2 size={16} />Run cleanup</button>}
            </div>
            {!cloud && (
              <DataList className="mt-5">
                <DataRow label="Database cleanup" value={cleanup ? cleanupSummary(cleanup.database) : "loading"} />
                <DataRow label="Docker keep set" value={cleanup ? `${cleanup.docker.keepContainers} containers, ${cleanup.docker.keepImages} images` : "loading"} />
                <DataRow label="Docker cleanup job" value={cleanup?.docker.jobWillRun ? "available" : "not available"} />
              </DataList>
            )}
            {cloud && <Notice tone="neutral" className="mt-5" description="Global cleanup and Hostlet update actions are operator-managed in Hostlet Cloud. Customer-visible jobs remain scoped to your apps." />}
            {operationsMessage && <Notice tone={operationsMessage.toLowerCase().includes("failed") ? "danger" : "neutral"} className="mt-3" description={operationsMessage} />}

            <div className="mt-6 grid gap-4 lg:grid-cols-2">
              <div>
                <h3 className="text-sm font-semibold uppercase tracking-wide text-muted">Recent jobs</h3>
                <div className="mt-3 grid gap-2">
                  {jobs.slice(0, 8).map((job) => (
                    <div key={job.id} className="flex flex-wrap items-center justify-between gap-3 border-b border-line pb-2">
                      <div className="min-w-0">
                        <div className="truncate font-medium">{job.type}</div>
                        <div className="muted text-sm">{new Date(job.createdAt).toLocaleString()} · attempt {job.attempt}/{job.maxAttempts}</div>
                        {job.failure && <Notice tone="danger" className="mt-2" description={job.failure} />}
                      </div>
                      <div className="flex items-center gap-2">
                        <StatusPill status={job.status} />
                        {["failed", "expired", "cancelled"].includes(job.status) && <button className="button-secondary compact" onClick={() => retryJob(job.id)}>Retry</button>}
                        {job.status === "queued" && <button className="button-secondary compact" onClick={() => cancelJob(job.id)}>Cancel</button>}
                      </div>
                    </div>
                  ))}
                  {!jobs.length && <p className="muted">No agent jobs yet.</p>}
                </div>
              </div>
              <div>
                <h3 className="text-sm font-semibold uppercase tracking-wide text-muted">Audit trail</h3>
                <div className="mt-3 grid gap-2">
                  {audit.slice(0, 8).map((event) => (
                    <div key={event.id} className="border-b border-line pb-2">
                      <div className="font-medium">{event.eventType}</div>
                      <div className="muted text-sm">{event.actorType} · {new Date(event.createdAt).toLocaleString()}</div>
                    </div>
                  ))}
                  {!audit.length && <p className="muted">No audit events yet.</p>}
                </div>
              </div>
            </div>
          </Panel>
    </AppShell>
  );
}

function cleanupSummary(values: Record<string, number>) {
  const total = Object.values(values).reduce((sum, value) => sum + value, 0);
  return total ? `${total} old records` : "nothing eligible";
}

function formatBackupDate(value: string) {
  const isoLike = value.replace(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/, "$1-$2-$3T$4:$5:$6Z");
  const date = new Date(isoLike);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function updateMigrationSummary(update?: UpdatePayload | null) {
  if (!update) return "not checked";
  const parts = [];
  if (update.composeMigrations) parts.push("Compose");
  if (update.databaseMigrations) parts.push("database");
  return parts.length ? parts.join(" + ") : "none flagged";
}

function StatusCard({
  icon: Icon,
  title,
  status,
  message,
  rows,
  actionLabel,
  onAction,
}: {
  icon: LucideIcon;
  title: string;
  status: string;
  message: string;
  rows: Array<[string, string]>;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <Panel>
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <IconFrame icon={Icon} />
          <div>
            <h2 className="font-semibold">{title}</h2>
            <p className="muted mt-1">{message}</p>
          </div>
        </div>
        <StatusPill status={status} />
      </div>
      <DataList className="mt-5">
        {rows.map(([label, value]) => (
          <DataRow key={label} label={label} value={value} />
        ))}
      </DataList>
      {actionLabel && onAction && <button className="mt-4" onClick={onAction}>{actionLabel}</button>}
    </Panel>
  );
}

function Info({ icon: Icon, title, value }: { icon: LucideIcon; title: string; value: string }) {
  return (
    <Panel muted>
      <div className="flex items-center gap-2">
        <Icon size={18} />
        <div className="font-medium">{title}</div>
      </div>
      <p className="muted mt-2">{value}</p>
    </Panel>
  );
}
