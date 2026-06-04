"use client";

import type { LucideIcon } from "lucide-react";
import { Cloud, Download, GitBranch, KeyRound, Link2, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import { DataList, DataRow, IconFrame, Notice, Panel, StatusPill } from "@/components/ui";
import { GitHubDeviceFlow } from "@/components/GitHubDeviceFlow";
import { formatTimestamp } from "@/lib/time";
import type {
  AgentJob,
  AuditEvent,
  BackupMetadata,
  CleanupPlan,
  StatusMessage,
  StatusPayload,
  UpdatePayload,
  VersionPayload,
} from "./settings-data";

export function cleanupSummary(values: Record<string, number>) {
  const total = Object.values(values).reduce((sum, value) => sum + value, 0);
  return total ? `${total} old records` : "nothing eligible";
}

export function formatBackupDate(value: string) {
  const isoLike = value.replace(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/, "$1-$2-$3T$4:$5:$6Z");
  const date = new Date(isoLike);
  return Number.isNaN(date.getTime()) ? value : formatTimestamp(date);
}

export function updateMigrationSummary(update?: UpdatePayload | null) {
  if (!update) return "not checked";
  const items = [];
  if (update.composeMigrations) items.push("compose");
  if (update.databaseMigrations) items.push("database");
  return items.length ? items.join(", ") : "none";
}

export function ConnectionsSection({ github, cloudflare }: { github: StatusPayload | null; cloudflare: StatusPayload | null }) {
  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <div className="space-y-3">
        <StatusCard
          icon={GitBranch}
          title="GitHub"
          status={github?.tokenValid ? "connected" : github?.oauthConfigured ? "needs attention" : "not configured"}
          message={github?.message || "Loading GitHub status..."}
          rows={[
            ["Device Flow", github?.oauthConfigured ? "configured" : "missing"],
            ["Webhook secret", github?.webhookConfigured ? "configured" : "missing"],
            ["Account", github?.login || (github?.authenticated ? "signed in" : "not signed in")],
          ]}
        />
        {github?.oauthConfigured && github?.tokenValid !== true && <GitHubDeviceFlow buttonLabel="Reconnect GitHub" />}
      </div>
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
    </div>
  );
}

export function UpdatesSection({
  version,
  backup,
  message,
  onCheckForUpdates,
}: {
  version: VersionPayload | null;
  backup: BackupMetadata | null;
  message: StatusMessage;
  onCheckForUpdates: () => void;
}) {
  return (
    <Panel className="mt-6">
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
        <DataRow label="Last checked" value={version?.update?.checkedAt ? formatTimestamp(version.update.checkedAt) : "not checked"} />
        <DataRow label="Update command" value="hostlet update" />
        <DataRow label="Latest backup" value={backup?.created_at ? `${formatBackupDate(backup.created_at)}${backup.scheduled === "true" ? " (scheduled)" : ""}` : "not recorded"} />
      </DataList>
      <div className="mt-4 flex flex-wrap gap-2">
        <button className="button-secondary" onClick={onCheckForUpdates} disabled={version?.updateChecksEnabled === false}>
          <RefreshCw size={16} />
          Check for updates
        </button>
        {version?.update?.releaseNotesUrl && (
          <a className="button-secondary" href={version.update.releaseNotesUrl} target="_blank" rel="noreferrer">
            Release notes
          </a>
        )}
      </div>
      {message.text && <Notice tone={message.tone} className="mt-3" description={message.text} />}
      {version?.update?.unsupportedDirectUpdate && <Notice tone="danger" className="mt-3" description="This release does not support a direct update from the installed version. Read the release notes before upgrading." />}
      {version?.updateChecksEnabled === false && <Notice tone="neutral" className="mt-3" description="Update checks are disabled by HOSTLET_UPDATE_CHECKS=false." />}
    </Panel>
  );
}

export function AccessSummarySection() {
  return (
    <section className="mt-6 grid gap-4 md:grid-cols-3">
      <Info icon={ShieldCheck} title="Control-plane access" value="Password + GitHub Device Flow" />
      <Info icon={KeyRound} title="Secrets" value="Encrypted app env vars" />
      <Info icon={Link2} title="App exposure" value="Private by default, public under base domain" />
    </section>
  );
}

export function OperationsSection({
  cleanup,
  jobs,
  audit,
  message,
  onRunCleanup,
  onRetryJob,
  onCancelJob,
}: {
  cleanup: CleanupPlan | null;
  jobs: AgentJob[];
  audit: AuditEvent[];
  message: StatusMessage;
  onRunCleanup: () => void;
  onRetryJob: (id: string) => void;
  onCancelJob: (id: string) => void;
}) {
  return (
    <Panel className="mt-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <IconFrame icon={Trash2} />
          <div>
            <h2 className="font-semibold">Cleanup and operations</h2>
            <p className="muted mt-1">Preview retention counts, start cleanup, and recover recent durable jobs.</p>
          </div>
        </div>
        <button className="button-secondary" onClick={onRunCleanup}><Trash2 size={16} />Run cleanup</button>
      </div>
      <DataList className="mt-5">
        <DataRow label="Database cleanup" value={cleanup ? cleanupSummary(cleanup.database) : "loading"} />
        <DataRow label="Docker keep set" value={cleanup ? `${cleanup.docker.keepContainers} containers, ${cleanup.docker.keepImages} images` : "loading"} />
        <DataRow label="Docker cleanup job" value={cleanup?.docker.jobWillRun ? "available" : "not available"} />
      </DataList>
      {message.text && <Notice tone={message.tone} className="mt-3" description={message.text} />}

      <div className="mt-6 grid gap-4 lg:grid-cols-2">
        <JobsList jobs={jobs} onRetryJob={onRetryJob} onCancelJob={onCancelJob} />
        <AuditTrail audit={audit} />
      </div>
    </Panel>
  );
}

function JobsList({ jobs, onRetryJob, onCancelJob }: { jobs: AgentJob[]; onRetryJob: (id: string) => void; onCancelJob: (id: string) => void }) {
  return (
    <div>
      <h3 className="text-sm font-semibold uppercase tracking-wide text-muted">Recent jobs</h3>
      <div className="mt-3 grid gap-2">
        {jobs.slice(0, 8).map((job) => (
          <div key={job.id} className="flex flex-wrap items-center justify-between gap-3 border-b border-line pb-2">
            <div className="min-w-0">
              <div className="truncate font-medium">{job.type}</div>
              <div className="muted text-sm">{formatTimestamp(job.createdAt)} · attempt {job.attempt}/{job.maxAttempts}</div>
              {job.failure && <Notice tone="danger" className="mt-2" description={job.failure} />}
            </div>
            <div className="flex items-center gap-2">
              <StatusPill status={job.status} />
              {["failed", "expired", "cancelled"].includes(job.status) && <button className="button-secondary compact" onClick={() => onRetryJob(job.id)}>Retry</button>}
              {job.status === "queued" && <button className="button-secondary compact" onClick={() => onCancelJob(job.id)}>Cancel</button>}
            </div>
          </div>
        ))}
        {!jobs.length && <p className="muted">No agent jobs yet.</p>}
      </div>
    </div>
  );
}

function AuditTrail({ audit }: { audit: AuditEvent[] }) {
  return (
    <div>
      <h3 className="text-sm font-semibold uppercase tracking-wide text-muted">Audit trail</h3>
      <div className="mt-3 grid gap-2">
        {audit.slice(0, 8).map((event) => (
          <div key={event.id} className="border-b border-line pb-2">
            <div className="font-medium">{event.eventType}</div>
            <div className="muted text-sm">{event.actorType} · {formatTimestamp(event.createdAt)}</div>
          </div>
        ))}
        {!audit.length && <p className="muted">No audit events yet.</p>}
      </div>
    </div>
  );
}

function StatusCard({
  icon,
  title,
  status,
  message,
  rows,
}: {
  icon: LucideIcon;
  title: string;
  status: string;
  message: string;
  rows: Array<[string, string]>;
}) {
  return (
    <Panel>
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <IconFrame icon={icon} />
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
    </Panel>
  );
}

function Info({ icon, title, value }: { icon: LucideIcon; title: string; value: string }) {
  return (
    <Panel>
      <div className="flex items-center gap-3">
        <IconFrame icon={icon} />
        <div>
          <div className="text-sm font-semibold">{title}</div>
          <div className="muted mt-1 text-sm">{value}</div>
        </div>
      </div>
    </Panel>
  );
}
