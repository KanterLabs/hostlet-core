"use client";

import { useEffect, useState } from "react";
import type { LucideIcon } from "lucide-react";
import { Cloud, GitBranch, KeyRound, Link2, RefreshCw, ShieldCheck } from "lucide-react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";
import { PageHeader, StatusPill } from "@/components/ui";
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
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
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
                ["Managed prefix", cloudflare?.domainPrefix || "hostlet-"],
                ["Tunnel target", cloudflare?.tunnelTargetConfigured ? "configured" : "missing"],
              ]}
            />
          </div>

          <section className="mt-6 grid gap-4 md:grid-cols-3">
            <Info icon={ShieldCheck} title="Control-plane access" value="Password + GitHub Device Flow" />
            <Info icon={KeyRound} title="Secrets" value="Encrypted app env vars" />
            <Info icon={Link2} title="App exposure" value="Private by default" />
          </section>
        </div>
      </section>
    </main>
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
    <section className="panel p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-surface-alt ring-1 ring-line">
            <Icon size={20} />
          </div>
          <div>
            <h2 className="font-semibold">{title}</h2>
            <p className="muted mt-1">{message}</p>
          </div>
        </div>
        <StatusPill status={status} />
      </div>
      <div className="mt-5 grid gap-2">
        {rows.map(([label, value]) => (
          <div key={label} className="flex items-center justify-between gap-4 rounded-md bg-surface-alt px-3 py-2 text-sm">
            <span className="text-muted">{label}</span>
            <span className="break-all text-right font-medium">{value}</span>
          </div>
        ))}
      </div>
      {actionLabel && onAction && <button className="mt-4" onClick={onAction}>{actionLabel}</button>}
    </section>
  );
}

function Info({ icon: Icon, title, value }: { icon: LucideIcon; title: string; value: string }) {
  return (
    <div className="panel-muted p-4">
      <div className="flex items-center gap-2">
        <Icon size={18} />
        <div className="font-medium">{title}</div>
      </div>
      <p className="muted mt-2">{value}</p>
    </div>
  );
}
