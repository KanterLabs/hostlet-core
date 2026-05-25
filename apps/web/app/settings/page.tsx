"use client";

import { useEffect, useState } from "react";
import type { LucideIcon } from "lucide-react";
import { Cloud, Download, GitBranch, KeyRound, Link2, RefreshCw, ShieldCheck } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, DataList, DataRow, IconFrame, PageHeader, Panel, StatusPill } from "@/components/ui";
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

export default function Settings() {
  const [github, setGithub] = useState<StatusPayload | null>(null);
  const [cloudflare, setCloudflare] = useState<StatusPayload | null>(null);
  const [version, setVersion] = useState<VersionPayload | null>(null);
  const [updateMessage, setUpdateMessage] = useState("");

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
    api<StatusPayload>("/api/github/status").then(setGithub).catch((error) => setGithub({ message: error instanceof Error ? error.message : "Could not load GitHub status." }));
    api<StatusPayload>("/api/cloudflare/status").then(setCloudflare).catch((error) => setCloudflare({ message: error instanceof Error ? error.message : "Could not load Cloudflare status." }));
    api<VersionPayload>("/api/system/version").then(setVersion).catch(() => setVersion(null));
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

  return (
    <AppShell>
          <PageHeader
            eyebrow="Control plane"
            title="Settings"
            description="Connection status for GitHub auth, webhooks, Cloudflare DNS, and public app routing."
            actions={<button className="button-secondary" onClick={refresh}><RefreshCw size={16} />Refresh</button>}
          />

          <WebhookNotice className="mb-6" />

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
              <DataRow label="Last checked" value={version?.update?.checkedAt ? new Date(version.update.checkedAt).toLocaleString() : "not checked"} />
              <DataRow label="Update command" value="hostlet update" />
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
            {updateMessage && <p className="muted mt-3">{updateMessage}</p>}
            {version?.update?.unsupportedDirectUpdate && <p className="mt-3 text-sm text-red-700">This release does not support a direct update from the installed version. Read the release notes before upgrading.</p>}
            {version?.updateChecksEnabled === false && <p className="muted mt-3">Update checks are disabled by HOSTLET_UPDATE_CHECKS=false.</p>}
          </Panel>

          <section className="mt-6 grid gap-4 md:grid-cols-3">
            <Info icon={ShieldCheck} title="Control-plane access" value="Password + GitHub Device Flow" />
            <Info icon={KeyRound} title="Secrets" value="Encrypted app env vars" />
            <Info icon={Link2} title="App exposure" value="Private by default, public under base domain" />
          </section>
    </AppShell>
  );
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
