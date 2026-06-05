"use client";

import { useEffect, useState } from "react";
import { api, apiUrl } from "@/lib/api";
import { useVisibilityPoll } from "@/lib/useVisibilityPoll";

export type SocketState = "connecting" | "connected" | "reconnecting" | "closed";

export type DeploymentLogLine = {
  stream: string;
  line: string;
};

export type BaseDeployment = {
  id: string;
  status: string;
  failure?: string | null;
};

export type DeploymentLogs<TDeployment> = {
  deployment: TDeployment | null;
  logs: string[];
  socketState: SocketState;
  socketMessage: string;
};

const DEPLOYMENT_POLL_MS = 2500;
const SOCKET_RETRY_MS = 2000;
const MAX_LOG_LINES = 1000;

const formatLogLine = (row: DeploymentLogLine) => `${row.stream}: ${row.line}`;

export function useDeploymentLogs<TDeployment extends BaseDeployment>(
  id: string,
): DeploymentLogs<TDeployment> {
  const [deployment, setDeployment] = useState<TDeployment | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [socketState, setSocketState] = useState<SocketState>("connecting");
  const [socketMessage, setSocketMessage] = useState("");

  useVisibilityPoll(
    async ({ isActive }) => {
      try {
        const loaded = await api<TDeployment>(`/api/deployments/${id}`);
        if (isActive()) setDeployment(loaded);
      } catch {
        if (isActive()) {
          setDeployment({
            id,
            status: "unknown",
            failure: "Deployment could not be loaded.",
          } as TDeployment);
        }
      }
    },
    { intervalMs: DEPLOYMENT_POLL_MS, deps: [id] },
  );

  useEffect(() => {
    let active = true;
    api<DeploymentLogLine[]>(`/api/deployments/${id}/logs`)
      .then((rows) => {
        if (active) setLogs(rows.map(formatLogLine));
      })
      .catch(() => {});

    let closed = false;
    let retry: number | undefined;
    let ws: WebSocket | undefined;
    const connect = () => {
      setSocketState((current) => (current === "closed" ? "connecting" : current));
      ws = new WebSocket(`${apiUrl().replace("http", "ws")}/ws/logs/${id}`);
      ws.onopen = () => {
        if (!active) return;
        setSocketState("connected");
        setSocketMessage("");
      };
      ws.onmessage = (event) => {
        if (!active) return;
        try {
          const row = JSON.parse(event.data) as DeploymentLogLine;
          setLogs((current) => [...current, formatLogLine(row)].slice(-MAX_LOG_LINES));
        } catch {
          setSocketMessage("A log event could not be parsed.");
        }
      };
      ws.onerror = () => {
        if (!active) return;
        setSocketMessage("Live log connection had an error.");
      };
      ws.onclose = () => {
        if (closed) return;
        setSocketState("reconnecting");
        setSocketMessage("Live logs disconnected. Reconnecting...");
        retry = window.setTimeout(connect, SOCKET_RETRY_MS);
      };
    };
    connect();

    return () => {
      active = false;
      closed = true;
      if (retry) window.clearTimeout(retry);
      ws?.close();
    };
  }, [id]);

  return { deployment, logs, socketState, socketMessage };
}
