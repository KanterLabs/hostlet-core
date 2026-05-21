"use client";

import { useEffect, useState } from "react";
import { CheckCircle2, Clipboard, ExternalLink, GitBranch, Loader2, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";

type DeviceStart = {
  flowId: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete?: string | null;
  expiresIn: number;
  interval: number;
};

type DevicePoll = {
  status: "pending" | "authorized" | "expired" | "denied";
  message: string;
  interval?: number;
  login?: string;
  redirectTo?: string;
};

export function GitHubDeviceFlow({
  buttonLabel = "Connect GitHub",
  className = "",
  fullWidth = false,
  onAuthorized,
}: {
  buttonLabel?: string;
  className?: string;
  fullWidth?: boolean;
  onAuthorized?: () => void;
}) {
  const [flow, setFlow] = useState<DeviceStart | null>(null);
  const [status, setStatus] = useState<DevicePoll["status"] | "idle" | "loading" | "error">("idle");
  const [message, setMessage] = useState("");
  const [copied, setCopied] = useState(false);

  async function start() {
    setStatus("loading");
    setMessage("");
    setCopied(false);
    try {
      const next = await api<DeviceStart>("/auth/github/device/start", {
        method: "POST",
        body: "{}",
      });
      setFlow(next);
      setStatus("pending");
      setMessage("Waiting for GitHub authorization.");
    } catch (error) {
      setFlow(null);
      setStatus("error");
      setMessage(error instanceof Error ? error.message : "Could not start GitHub Device Flow.");
    }
  }

  useEffect(() => {
    if (!flow || status !== "pending") return;
    const activeFlow = flow;
    let cancelled = false;
    let timeout: number | undefined;

    async function poll() {
      try {
        const result = await api<DevicePoll>("/auth/github/device/poll", {
          method: "POST",
          body: JSON.stringify({ flow_id: activeFlow.flowId }),
        });
        if (cancelled) return;
        setMessage(result.message);
        if (result.interval && result.interval !== activeFlow.interval) {
          setFlow({ ...activeFlow, interval: result.interval });
          return;
        }
        if (result.status === "authorized") {
          setStatus("authorized");
          onAuthorized?.();
          window.setTimeout(() => window.location.assign(result.redirectTo || "/"), 500);
          return;
        }
        if (result.status === "expired" || result.status === "denied") {
          setStatus(result.status);
          return;
        }
        timeout = window.setTimeout(poll, Math.max(result.interval || activeFlow.interval, 5) * 1000);
      } catch (error) {
        if (!cancelled) {
          setStatus("error");
          setMessage(error instanceof Error ? error.message : "Could not poll GitHub authorization.");
        }
      }
    }

    timeout = window.setTimeout(poll, Math.max(activeFlow.interval, 5) * 1000);
    return () => {
      cancelled = true;
      if (timeout) window.clearTimeout(timeout);
    };
  }, [flow, status, onAuthorized]);

  async function copyCode() {
    if (!flow) return;
    try {
      await navigator.clipboard.writeText(flow.userCode);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setMessage(flow.userCode);
    }
  }

  const busy = status === "loading";
  const completeUrl = flow?.verificationUriComplete || flow?.verificationUri;

  if (!flow) {
    return (
      <div className={className}>
        <button className={fullWidth ? "w-full" : ""} onClick={start} disabled={busy}>
          {busy ? <Loader2 size={16} className="animate-spin" /> : <GitBranch size={16} />}
          {busy ? "Starting..." : buttonLabel}
        </button>
        {message && <p className="mt-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800">{message}</p>}
      </div>
    );
  }

  return (
    <div className={`space-y-3 ${className}`}>
      <div className="rounded-md border border-line bg-white p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="eyebrow">GitHub code</div>
            <div className="mt-1 font-mono text-3xl font-semibold tracking-[0.16em] text-ink">{flow.userCode}</div>
          </div>
          <div className="flex flex-wrap gap-2">
            <button className="button-secondary" type="button" onClick={copyCode}>
              <Clipboard size={16} />
              {copied ? "Copied" : "Copy"}
            </button>
            <a className="button" href={completeUrl} target="_blank" rel="noreferrer">
              <ExternalLink size={16} />
              Open GitHub
            </a>
          </div>
        </div>
      </div>
      <div className="flex items-center gap-2 text-sm text-neutral-700">
        {status === "authorized" ? (
          <CheckCircle2 size={16} className="text-emerald-700" />
        ) : status === "pending" ? (
          <Loader2 size={16} className="animate-spin text-amber-700" />
        ) : (
          <RefreshCw size={16} className="text-red-700" />
        )}
        <span>{message || "Waiting for GitHub authorization."}</span>
      </div>
      {(status === "expired" || status === "denied" || status === "error") && (
        <button className="button-secondary" type="button" onClick={start}>
          <RefreshCw size={16} />
          Start again
        </button>
      )}
    </div>
  );
}
