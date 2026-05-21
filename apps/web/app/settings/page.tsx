"use client";

import { useEffect, useState } from "react";
import { Nav } from "@/components/Nav";
import { api, apiUrl } from "@/lib/api";

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
    api<StatusPayload>("/api/github/status").then(setGithub).catch((e) => setGithub({ message: e instanceof Error ? e.message : "Could not load GitHub status." }));
    api<StatusPayload>("/api/cloudflare/status").then(setCloudflare).catch((e) => setCloudflare({ message: e instanceof Error ? e.message : "Could not load Cloudflare status." }));
  }, []);

  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <h1 className="text-2xl font-semibold">Settings</h1>
        <div className="mt-6 grid gap-4 lg:grid-cols-2">
          <StatusCard
            title="GitHub"
            status={github?.tokenValid ? "connected" : github?.oauthConfigured ? "needs attention" : "not configured"}
            message={github?.message || "Loading GitHub status..."}
            rows={[
              ["OAuth", github?.oauthConfigured ? "configured" : "missing"],
              ["Webhook secret", github?.webhookConfigured ? "configured" : "missing"],
              ["Account", github?.login || (github?.authenticated ? "signed in" : "not signed in")],
            ]}
            actionLabel="Reconnect GitHub"
            onAction={() => window.location.assign(`${apiUrl()}/auth/github/start`)}
          />
          <StatusCard
            title="Cloudflare"
            status={cloudflare?.tokenValid ? "connected" : cloudflare?.configured ? "needs attention" : "not configured"}
            message={cloudflare?.message || "Loading Cloudflare status..."}
            rows={[
              ["Base domain", cloudflare?.baseDomain || "missing"],
              ["Prefix", cloudflare?.domainPrefix || "hostlet-"],
              ["Tunnel target", cloudflare?.tunnelTargetConfigured ? "configured" : "missing"],
            ]}
          />
        </div>
      </section>
    </main>
  );
}

function StatusCard({ title, status, message, rows, actionLabel, onAction }: { title: string; status: string; message: string; rows: Array<[string, string]>; actionLabel?: string; onAction?: () => void }) {
  return (
    <div className="rounded-lg border border-line bg-white p-4">
      <div className="flex items-center justify-between gap-3">
        <h2 className="font-medium">{title}</h2>
        <span className="rounded-full bg-panel px-2 py-1 text-xs text-neutral-700 ring-1 ring-line">{status}</span>
      </div>
      <p className="mt-3 text-sm text-neutral-700">{message}</p>
      <div className="mt-4 grid gap-2">
        {rows.map(([label, value]) => (
          <div key={label} className="flex items-center justify-between gap-4 rounded-md bg-panel px-3 py-2 text-sm">
            <span className="text-neutral-500">{label}</span>
            <span className="font-medium">{value}</span>
          </div>
        ))}
      </div>
      {actionLabel && onAction && <button className="mt-4" onClick={onAction}>{actionLabel}</button>}
    </div>
  );
}
