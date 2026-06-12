"use client";

import { useState } from "react";
import { Copy, GitBranch, Globe2, Play } from "lucide-react";
import { webhookReadiness } from "@/lib/webhooks";
import { StatusPill } from "@/components/ui";

const COPY_IDLE_LABEL = "Copy payload URL";
const COPY_RESET_MS = 1400;

/** Tailwind classes for the ready (public webhook) vs not-ready (LAN/manual) variants. */
const READY_TONE = {
  card: "border-emerald-200 bg-emerald-50",
  icon: "text-emerald-700",
  heading: "font-semibold text-emerald-950",
  body: "text-emerald-900",
};
const NOT_READY_TONE = {
  card: "border-amber-200 bg-amber-50",
  icon: "text-amber-700",
  heading: "font-semibold text-amber-950",
  body: "text-amber-900",
};

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
  const [copyState, setCopyState] = useState(COPY_IDLE_LABEL);
  const readiness = webhookReadiness();
  const ready = readiness.canReceiveGitHub;
  const tone = ready ? READY_TONE : NOT_READY_TONE;
  const title = ready ? "GitHub webhooks can reach Hostlet" : "LAN/manual mode";
  const statusLabel = ready ? "public webhook URL" : "manual deploy after push";
  const description = ready
    ? "Use this payload URL in the GitHub repo webhook, set the secret to GITHUB_WEBHOOK_SECRET, and enable auto redeploy on the app."
    : `GitHub cannot deliver push webhooks because ${readiness.reason}. Push to GitHub, then click Deploy latest in Hostlet.`;
  const autoDeployWarning = autoDeployEnabled && !ready
    ? "Auto redeploy is enabled for this app, but pushes will not start deployments until PUBLIC_WEBHOOK_URL or PUBLIC_API_URL is a public HTTPS URL."
    : "";

  function flashCopyState(label: string) {
    setCopyState(label);
    window.setTimeout(() => setCopyState(COPY_IDLE_LABEL), COPY_RESET_MS);
  }

  async function copyWebhookUrl() {
    try {
      await navigator.clipboard.writeText(readiness.webhookUrl);
      flashCopyState("Copied");
    } catch {
      flashCopyState("Copy failed");
    }
  }

  return (
    <section className={`rounded-lg border p-4 ${tone.card} ${className}`}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            {ready ? <Globe2 size={18} className={tone.icon} /> : <GitBranch size={18} className={tone.icon} />}
            <h2 className={tone.heading}>{title}</h2>
            <StatusPill status={ready ? "connected" : "needs attention"} label={statusLabel} />
          </div>
          <p className={`mt-2 max-w-2xl text-sm ${tone.body}`}>{description}</p>
          {autoDeployWarning && <p className="mt-2 max-w-2xl text-sm font-medium text-amber-950">{autoDeployWarning}</p>}
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
            <button type="button" className="button" disabled={deployDisabled} onClick={onManualDeploy}>
              <Play size={16} />
              Deploy latest
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
