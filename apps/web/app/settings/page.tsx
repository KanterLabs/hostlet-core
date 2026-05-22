"use client";

import { useEffect, useState } from "react";
import type { LucideIcon } from "lucide-react";
import { Cloud, GitBranch, KeyRound, Link2, RefreshCw, ShieldCheck } from "lucide-react";
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

export default function Settings() {
  const [github, setGithub] = useState<StatusPayload | null>(null);
  const [cloudflare, setCloudflare] = useState<StatusPayload | null>(null);

  useEffect(() => {
    refresh();
  }, []);

  function refresh() {
    api<StatusPayload>("/api/github/status").then(setGithub).catch((error) => setGithub({ message: error instanceof Error ? error.message : "Could not load GitHub status." }));
    api<StatusPayload>("/api/cloudflare/status").then(setCloudflare).catch((error) => setCloudflare({ message: error instanceof Error ? error.message : "Could not load Cloudflare status." }));
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

          <section className="mt-6 grid gap-4 md:grid-cols-3">
            <Info icon={ShieldCheck} title="Control-plane access" value="Password + GitHub Device Flow" />
            <Info icon={KeyRound} title="Secrets" value="Encrypted app env vars" />
            <Info icon={Link2} title="App exposure" value="Private by default, public under base domain" />
          </section>
    </AppShell>
  );
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
