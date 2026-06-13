"use client";

import { useEffect, useState } from "react";
import { useTimedReset } from "@/lib/useTimedReset";
import { CheckCircle2, Clipboard, ExternalLink, GitBranch, Loader2, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import { Notice } from "@/components/ui";

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

/** UI lifecycle states layered on top of the server-reported device-flow status. */
type FlowState = DevicePoll["status"] | "idle" | "loading" | "error";

/** Delay before redirecting after authorization, giving the success state a beat to render. */
const REDIRECT_DELAY_MS = 500;
/** How long the "Copied" confirmation stays visible before reverting to "Copy". */
const COPY_RESET_MS = 1600;
/** GitHub device-flow minimum poll interval (seconds); we never poll faster than this. */
const MIN_POLL_INTERVAL_S = 5;

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
  const [status, setStatus] = useState<FlowState>("idle");
  const [message, setMessage] = useState("");
  const [copied, flashCopied] = useTimedReset(false, COPY_RESET_MS);
  // Captured from the server response; consumed by the dedicated redirect effect.
  const [redirectTo, setRedirectTo] = useState<string | undefined>(undefined);

  async function start() {
    setStatus("loading");
    setMessage("");
    flashCopied(false);
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

    function scheduleNext(intervalSeconds: number | undefined) {
      const seconds = Math.max(intervalSeconds || activeFlow.interval, MIN_POLL_INTERVAL_S);
      timeout = window.setTimeout(poll, seconds * 1000);
    }

    function applyResult(result: DevicePoll) {
      setMessage(result.message);
      // A changed interval re-renders the flow, which restarts this effect with the new cadence.
      if (result.interval && result.interval !== activeFlow.interval) {
        setFlow({ ...activeFlow, interval: result.interval });
        return;
      }
      if (result.status === "authorized") {
        setRedirectTo(result.redirectTo);
        setStatus("authorized");
        onAuthorized?.();
        return;
      }
      if (result.status === "expired" || result.status === "denied") {
        setStatus(result.status);
        return;
      }
      scheduleNext(result.interval);
    }

    async function poll() {
      try {
        const result = await api<DevicePoll>("/auth/github/device/poll", {
          method: "POST",
          body: JSON.stringify({ flow_id: activeFlow.flowId }),
        });
        if (cancelled) return;
        applyResult(result);
      } catch (error) {
        if (!cancelled) {
          setStatus("error");
          setMessage(error instanceof Error ? error.message : "Could not poll GitHub authorization.");
        }
      }
    }

    scheduleNext(activeFlow.interval);
    return () => {
      cancelled = true;
      if (timeout) window.clearTimeout(timeout);
    };
  }, [flow, status, onAuthorized]);

  // Dedicated effect: arms the redirect timer only when status reaches "authorized".
  // Kept separate from the polling effect so its cleanup cannot cancel the timer
  // mid-flight when polling re-runs on status change.
  useEffect(() => {
    if (status !== "authorized") return;
    const id = window.setTimeout(() => window.location.assign(localRedirectPath(redirectTo)), REDIRECT_DELAY_MS);
    return () => window.clearTimeout(id);
  }, [status, redirectTo]);

  async function copyCode() {
    if (!flow) return;
    try {
      await navigator.clipboard.writeText(flow.userCode);
      flashCopied(true);
    } catch {
      setMessage(flow.userCode);
    }
  }

  const busy = status === "loading";
  const completeUrl = flow?.verificationUriComplete || flow?.verificationUri;

  if (!flow) {
    return (
      <div className={className}>
        <button className={fullWidth ? "button w-full" : "button"} onClick={start} disabled={busy}>
          {busy ? <Loader2 size={16} className="animate-spin" /> : <GitBranch size={16} />}
          {busy ? "Starting..." : buttonLabel}
        </button>
        {message && <Notice tone="danger" className="mt-3" description={message} />}
      </div>
    );
  }

  return (
    <div className={`space-y-3 ${className}`}>
      <div className="rounded-md border border-line bg-surface-alt p-4">
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
      <div className="flex items-center gap-2 text-sm text-muted">
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

function localRedirectPath(value?: string) {
  if (!value) return "/";
  try {
    const url = new URL(value, window.location.origin);
    if (url.origin !== window.location.origin) return "/";
    return `${url.pathname}${url.search}${url.hash}` || "/";
  } catch {
    return "/";
  }
}
