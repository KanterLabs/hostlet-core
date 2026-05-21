"use client";

import { useState } from "react";
import { Copy, GitBranch, Globe2, Play } from "lucide-react";
import { webhookReadiness } from "@/lib/webhooks";
import { StatusPill } from "@/components/ui";

export function WebhookNotice({
  autoDeployEnabled,
  onManualDeploy,
  deployDisabled,
  className = "",
}: {
  autoDeployEnabled?: boolean;
  onManualDeploy?: () => void;
  deployDisabled?: boolean;
  className?: string;
}) {
  const [copyState, setCopyState] = useState("Copy payload URL");
  const readiness = webhookReadiness();
  const ready = readiness.canReceiveGitHub;
  const title = ready ? "GitHub webhooks can reach Hostlet" : "LAN/manual mode";
  const statusLabel = ready ? "public webhook URL" : "manual deploy after push";
  const description = ready
    ? "Use this payload URL in the GitHub repo webhook, set the secret to GITHUB_WEBHOOK_SECRET, and enable auto redeploy on the app."
    : `GitHub cannot deliver push webhooks because ${readiness.reason}. Push to GitHub, then click Deploy latest in Hostlet.`;
  const autoDeployWarning = autoDeployEnabled && !ready
    ? "Auto redeploy is enabled for this app, but pushes will not start deployments until PUBLIC_WEBHOOK_URL or PUBLIC_API_URL is a public HTTPS URL."
    : "";

  async function copyWebhookUrl() {
    try {
      await navigator.clipboard.writeText(readiness.webhookUrl);
      setCopyState("Copied");
      window.setTimeout(() => setCopyState("Copy payload URL"), 1400);
    } catch {
      setCopyState("Copy failed");
      window.setTimeout(() => setCopyState("Copy payload URL"), 1400);
    }
  }

  return (
    <section className={`rounded-lg border p-4 ${ready ? "border-emerald-200 bg-emerald-50" : "border-amber-200 bg-amber-50"} ${className}`}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            {ready ? <Globe2 size={18} className="text-emerald-700" /> : <GitBranch size={18} className="text-amber-700" />}
            <h2 className={ready ? "font-semibold text-emerald-950" : "font-semibold text-amber-950"}>{title}</h2>
            <StatusPill status={ready ? "connected" : "needs attention"} label={statusLabel} />
          </div>
          <p className={`mt-2 max-w-3xl text-sm ${ready ? "text-emerald-900" : "text-amber-900"}`}>{description}</p>
          {autoDeployWarning && <p className="mt-2 text-sm font-medium text-amber-950">{autoDeployWarning}</p>}
          <div className="mt-3 break-all rounded-md bg-white/70 px-3 py-2 font-mono text-xs text-neutral-800 ring-1 ring-black/5">
            {readiness.webhookUrl}
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <button type="button" className="button-secondary bg-white/80" onClick={copyWebhookUrl}>
            <Copy size={16} />
            {copyState}
          </button>
          {!ready && onManualDeploy && (
            <button type="button" disabled={deployDisabled} onClick={onManualDeploy}>
              <Play size={16} />
              Deploy latest
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
