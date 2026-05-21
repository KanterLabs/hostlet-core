"use client";

import { useEffect, useState } from "react";
import { CheckCircle2, CircleAlert, CircleDashed } from "lucide-react";
import { api, apiUrl } from "@/lib/api";

type Status = {
  oauthConfigured: boolean;
  webhookConfigured: boolean;
  authenticated: boolean;
  tokenValid: boolean | null;
  login: string | null;
  message: string;
};

export function GitHubStatus({ compact = false }: { compact?: boolean }) {
  const [status, setStatus] = useState<Status | null>(null);

  useEffect(() => {
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

  const icon = !status ? <CircleDashed size={16} /> : status.oauthConfigured && (status.tokenValid === true || !status.authenticated)
    ? <CheckCircle2 size={16} />
    : <CircleAlert size={16} />;
  const tone = !status ? "border-line bg-panel text-neutral-700" : status.oauthConfigured && (status.tokenValid === true || !status.authenticated)
    ? "border-emerald-200 bg-emerald-50 text-emerald-900"
    : "border-red-200 bg-red-50 text-red-900";

  return (
    <div className={`rounded-md border p-3 text-sm ${tone}`}>
      <div className="flex items-center gap-2 font-medium">
        {icon}
        <span>{status?.tokenValid ? `GitHub connected as ${status.login}` : status?.oauthConfigured ? "GitHub OAuth configured" : "GitHub setup needed"}</span>
      </div>
      {!compact && <p className="mt-2">{status?.message || "Checking GitHub connection..."}</p>}
      {!compact && status && (
        <>
          <div className="mt-2 flex gap-2 text-xs">
            <span>OAuth: {status.oauthConfigured ? "ready" : "missing"}</span>
            <span>Webhook secret: {status.webhookConfigured ? "custom" : "dev/default"}</span>
          </div>
          {status.oauthConfigured && status.tokenValid !== true && (
            <a className="button mt-3 bg-ink hover:bg-neutral-800" href={`${apiUrl()}/auth/github/start`}>
              Connect GitHub
            </a>
          )}
        </>
      )}
    </div>
  );
}
