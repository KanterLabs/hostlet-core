"use client";

import { useEffect, useState } from "react";
import { api, apiUrl } from "@/lib/api";
import type { SocketState } from "./deploymentStatus";

type Deployment = {
  id: string;
  appId?: string;
  status: string;
  commitSha?: string | null;
  failure?: string | null;
  runtimeMetadata?: RuntimeMetadata | null;
};

type RuntimeMetadata = {
  packagingStrategy?: string | null;
  generatedDockerfile?: boolean | null;
  detectedFramework?: string | null;
  runtimeKind?: string | null;
  packageManager?: string | null;
  buildDurationMs?: number | null;
  imageSizeBytes?: number | null;
};

type LogLine = {
  stream: string;
  line: string;
};

export type { Deployment, RuntimeMetadata, LogLine };

// How often the deployment record is re-fetched while the view is open.
const DEPLOYMENT_POLL_MS = 2500;
// Delay before attempting to re-open the live-log WebSocket after a drop.
const SOCKET_RETRY_MS = 2000;
// Maximum number of live-log lines retained in memory.
const MAX_LOG_LINES = 1000;

const formatLogLine = (row: LogLine) => `${row.stream}: ${row.line}`;

export type DeploymentLogs = {
  deployment: Deployment | null;
  logs: string[];
  socketState: SocketState;
  socketMessage: string;
};

/**
 * Owns the deployment detail view's live-data lifecycle: it polls the
 * deployment record, loads the initial log buffer, and maintains a
 * self-reconnecting WebSocket that streams new log lines.
 */
export function useDeploymentLogs(id: string): DeploymentLogs {
  const [deployment, setDeployment] = useState<Deployment | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [socketState, setSocketState] = useState<SocketState>("connecting");
  const [socketMessage, setSocketMessage] = useState("");

  useEffect(() => {
    const loadDeployment = () =>
      api<Deployment>(`/api/deployments/${id}`)
        .then(setDeployment)
        .catch(() => setDeployment({ id, status: "unknown", failure: "Deployment could not be loaded." }));
    loadDeployment();
    const poll = setInterval(loadDeployment, DEPLOYMENT_POLL_MS);

    api<LogLine[]>(`/api/deployments/${id}/logs`)
      .then((rows) => setLogs(rows.map(formatLogLine)))
      .catch(() => {});

    let closed = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let ws: WebSocket | undefined;
    const connect = () => {
      setSocketState((current) => (current === "closed" ? "connecting" : current));
      ws = new WebSocket(`${apiUrl().replace("http", "ws")}/ws/logs/${id}`);
      ws.onopen = () => {
        setSocketState("connected");
        setSocketMessage("");
      };
      ws.onmessage = (event) => {
        try {
          const row = JSON.parse(event.data);
          setLogs((current) => [...current, formatLogLine(row)].slice(-MAX_LOG_LINES));
        } catch {
          setSocketMessage("A log event could not be parsed.");
        }
      };
      ws.onerror = () => {
        setSocketMessage("Live log connection had an error.");
      };
      ws.onclose = () => {
        if (closed) return;
        setSocketState("reconnecting");
        setSocketMessage("Live logs disconnected. Reconnecting...");
        retry = setTimeout(connect, SOCKET_RETRY_MS);
      };
    };
    connect();

    return () => {
      closed = true;
      clearInterval(poll);
      if (retry) clearTimeout(retry);
      ws?.close();
      setSocketState("closed");
    };
  }, [id]);

  return { deployment, logs, socketState, socketMessage };
}
