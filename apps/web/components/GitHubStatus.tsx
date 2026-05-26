"use client";

import { useEffect, useState } from "react";
import { CheckCircle2, CircleAlert, CircleDashed, GitBranch } from "lucide-react";
import { api } from "@/lib/api";
import { GitHubDeviceFlow } from "@/components/GitHubDeviceFlow";

type Status = {
  oauthConfigured: boolean;
  webhookConfigured: boolean;
  authenticated: boolean;
  tokenValid: boolean | null;
  login: string | null;
  message: string;
};
type Session = {
  mode: "self_hosted" | "cloud";
  cloud?: { githubInstalled: boolean } | null;
};

export function GitHubStatus({ compact = false, showConnect = true }: { compact?: boolean; showConnect?: boolean }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [session, setSession] = useState<Session | null>(null);

  useEffect(() => {
    api<Session>("/api/session").then(setSession).catch(() => setSession(null));
    api<Status>("/api/github/status").then(setStatus).catch(() => {
      setStatus({
        oauthConfigured: false,
        webhookConfigured: false,
        authenticated: false,
        tokenValid: false,
        login: null,
        message: "Could not reach the Hostlet API.",
      });
    });
  }, []);

  if (session?.mode === "cloud") {
    const installed = !!session.cloud?.githubInstalled;
    return (
      <div className={`rounded-lg border p-3 text-sm shadow-sm shadow-neutral-950/5 ${installed ? "border-emerald-200 bg-emerald-50 text-emerald-900" : "border-amber-200 bg-amber-50 text-amber-900"}`}>
        <div className="flex items-center gap-2 font-medium">
          <GitBranch size={16} />
          <span className="min-w-0 truncate">{installed ? "GitHub App installed" : "GitHub App required"}</span>
          <span className="ml-auto shrink-0">{installed ? <CheckCircle2 size={16} /> : <CircleAlert size={16} />}</span>
        </div>
        {!compact && <p className="mt-2">{installed ? "Hostlet Cloud can access selected repositories." : "Install the Hostlet GitHub App to deploy repositories."}</p>}
        {!compact && showConnect && !installed && <a className="button mt-3" href="/auth/github/install/start">Install GitHub App</a>}
      </div>
    );
  }

  const icon = !status ? <CircleDashed size={16} /> : status.oauthConfigured && (status.tokenValid === true || !status.authenticated)
    ? <CheckCircle2 size={16} />
    : <CircleAlert size={16} />;
  const tone = !status ? "border-line bg-surface-alt text-muted" : status.oauthConfigured && (status.tokenValid === true || !status.authenticated)
    ? "border-emerald-200 bg-emerald-50 text-emerald-900"
    : "border-red-200 bg-red-50 text-red-900";

  return (
    <div className={`rounded-lg border p-3 text-sm shadow-sm shadow-neutral-950/5 ${tone}`}>
      <div className="flex items-center gap-2 font-medium">
        <GitBranch size={16} />
        <span className="min-w-0 truncate">{status?.tokenValid ? `GitHub connected as ${status.login}` : status?.oauthConfigured ? "GitHub Device Flow ready" : "GitHub setup needed"}</span>
        <span className="ml-auto shrink-0">{icon}</span>
      </div>
      {!compact && <p className="mt-2">{status?.message || "Checking GitHub connection..."}</p>}
      {!compact && status && (
        <>
          <div className="mt-3 grid gap-2 sm:grid-cols-2">
            <div className="rounded-md bg-white/70 px-3 py-2 text-xs">Device Flow: {status.oauthConfigured ? "ready" : "missing"}</div>
            <div className="rounded-md bg-white/70 px-3 py-2 text-xs">Webhook secret: {status.webhookConfigured ? "custom" : "dev/default"}</div>
          </div>
          {showConnect && status.oauthConfigured && status.tokenValid !== true && <GitHubDeviceFlow className="mt-3" />}
        </>
      )}
    </div>
  );
}
