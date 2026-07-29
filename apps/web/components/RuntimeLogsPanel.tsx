"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshCw, ScrollText } from "lucide-react";
import { Notice, Panel, SectionHeader } from "@/components/ui";
import { api } from "@/lib/api";
import { formatTimestamp } from "@/lib/time";
import { useVisibilityPoll } from "@/lib/useVisibilityPoll";
import {
  runtimeLogTimestamp,
  unavailableRuntimeServices,
  type RuntimeLogSnapshot,
} from "@/lib/runtimeLogs";

type RuntimeLogLoadIntent = {
  appId: string;
  deploymentId: string;
  generation: number;
  isActive: () => boolean;
};

export function RuntimeLogsPanel({
  appId,
  deploymentId,
}: {
  appId: string;
  deploymentId?: string | null;
}) {
  const [snapshot, setSnapshot] = useState<RuntimeLogSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const requestGeneration = useRef(0);
  const inFlight = useRef(false);
  const pendingIntent = useRef<RuntimeLogLoadIntent | null>(null);

  const load = useCallback(async (isActive: () => boolean) => {
    if (!deploymentId) return;
    const intent: RuntimeLogLoadIntent = {
      appId,
      deploymentId,
      generation: requestGeneration.current,
      isActive,
    };
    // Coalesce Strict Mode replays, route changes, and manual refreshes behind
    // the active request. Starting a replacement request immediately would race
    // the API/agent single-flight guard and surface a misleading 429.
    if (inFlight.current) {
      pendingIntent.current = intent;
      if (isActive()) setLoading(true);
      return;
    }

    let nextIntent: RuntimeLogLoadIntent | null = intent;
    while (nextIntent) {
      if (
        !nextIntent.isActive()
        || nextIntent.generation !== requestGeneration.current
      ) {
        nextIntent = pendingIntent.current;
        pendingIntent.current = null;
        continue;
      }

      inFlight.current = true;
      setLoading(true);
      setError("");
      try {
        const next = await api<RuntimeLogSnapshot>(
          `/api/apps/${nextIntent.appId}/runtime-logs`,
        );
        if (
          nextIntent.isActive()
          && nextIntent.generation === requestGeneration.current
        ) {
          setSnapshot(next);
        }
      } catch (requestError) {
        if (
          nextIntent.isActive()
          && nextIntent.generation === requestGeneration.current
        ) {
          setError(
            requestError instanceof Error
              ? requestError.message
              : "Runtime logs are temporarily unavailable.",
          );
        }
      } finally {
        inFlight.current = false;
        if (
          nextIntent.isActive()
          && nextIntent.generation === requestGeneration.current
        ) {
          setLoading(false);
        }
      }

      nextIntent = pendingIntent.current;
      pendingIntent.current = null;
    }
  }, [appId, deploymentId]);

  useEffect(() => {
    requestGeneration.current += 1;
    pendingIntent.current = null;
    setSnapshot(null);
    setError("");
    setLoading(false);
    return () => {
      requestGeneration.current += 1;
      pendingIntent.current = null;
    };
  }, [appId, deploymentId]);

  useVisibilityPoll(
    ({ isActive }) => load(isActive),
    {
      intervalMs: 10_000,
      enabled: !!deploymentId,
      deps: [appId, deploymentId],
    },
  );

  const refresh = useCallback(() => load(() => true), [load]);

  const unavailable = unavailableRuntimeServices(snapshot?.unavailableServices || []);

  return (
    <Panel>
      <SectionHeader
        icon={ScrollText}
        title="Runtime logs"
        description="A fresh, bounded tail from each service in the current deployment. Runtime output is not archived."
        action={
          <>
            <span className="text-xs text-muted" role="status" aria-live="polite">
              {loading
                ? "Refreshing runtime logs."
                : snapshot?.capturedAt
                  ? `Captured ${formatTimestamp(snapshot.capturedAt, "time")}`
                  : ""}
            </span>
            <button
              type="button"
              className="button-secondary"
              disabled={!deploymentId || loading}
              onClick={refresh}
            >
              <RefreshCw size={15} className={loading ? "animate-spin" : ""} />
              {loading ? "Refreshing..." : "Refresh"}
            </button>
          </>
        }
      />

      {!deploymentId ? (
        <Notice
          tone="neutral"
          description="Deploy this app once to inspect its runtime output."
        />
      ) : error ? (
        <Notice
          tone="danger"
          title="Runtime logs unavailable."
          description={error}
          action={
            <button type="button" className="button-secondary" onClick={refresh}>
              <RefreshCw size={15} />Try again
            </button>
          }
        />
      ) : loading && !snapshot ? (
        <Notice tone="neutral" description="Requesting a fresh runtime log tail from the agent…" />
      ) : (
        <>
          {unavailable && (
            <Notice tone="warning" className="mb-3" description={unavailable} />
          )}
          {snapshot?.truncated && (
            <p className="mb-2 text-xs text-muted">
              Showing the newest output within the 500-line, 256 KiB limit.
            </p>
          )}
          <div
            className="min-h-[180px] max-h-[48vh] overflow-auto rounded-lg border border-neutral-800 bg-neutral-950 p-3 font-mono text-xs leading-5 text-green-100"
            role="region"
            aria-label="Runtime log output"
            aria-busy={loading}
            tabIndex={0}
          >
            {!snapshot || snapshot.lines.length === 0 ? (
              <div className="text-neutral-400">
                {unavailable ? "No service output was available." : "No runtime output yet."}
              </div>
            ) : (
              snapshot.lines.map((line, index) => (
                <div
                  key={`${line.timestamp || "untimed"}-${line.service}-${index}`}
                  className={`grid min-w-max grid-cols-[7.5rem_6rem_minmax(20rem,1fr)] gap-2 ${
                    line.stream === "stderr" ? "text-amber-200" : "text-green-100"
                  }`}
                >
                  <time
                    className="text-neutral-500"
                    dateTime={line.timestamp || undefined}
                    title={line.timestamp || undefined}
                  >
                    {runtimeLogTimestamp(line.timestamp)}
                  </time>
                  <span className="truncate text-sky-300" title={line.service}>
                    [{line.service}]
                  </span>
                  <span className="whitespace-pre-wrap break-words">{line.line}</span>
                </div>
              ))
            )}
          </div>
        </>
      )}
    </Panel>
  );
}
