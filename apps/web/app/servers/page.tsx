"use client";

import { useState } from "react";
import { HardDrive, Server } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, EmptyState, Metric, MetricsGrid, PageHeader, Panel, StatusPill } from "@/components/ui";
import {
  SERVERS_POLL_INTERVAL_MS,
  deriveServerCounts,
  formatLastSeen,
  useVisibilityPoll,
  type ServerRow,
} from "./servers-data";

export default function Servers() {
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [emptyMessage, setEmptyMessage] = useState("Loading machines...");
  const [error, setError] = useState("");

  useVisibilityPoll(loadServers, SERVERS_POLL_INTERVAL_MS);

  function loadServers() {
    api<ServerRow[]>("/api/servers")
      .then((rows) => {
        setServers(rows);
        setError("");
        setEmptyMessage(rows.length ? "" : "No machines yet.");
      })
      .catch((err) => setError(`Could not load machines. ${err instanceof Error ? err.message : "Sign in again."}`));
  }

  const { online, local } = deriveServerCounts(servers);

  return (
    <AppShell>
          <PageHeader
            eyebrow="Machines"
            title="This machine"
            description="Hostlet deploys apps onto the same machine that runs this control plane."
          />

          <MetricsGrid columns="md:grid-cols-3">
            <Metric label="Local machines" value={String(local)} detail="current deploy target" icon={Server} />
            <Metric label="Online agents" value={`${online}/${servers.length || 1}`} detail="websocket heartbeat" icon={HardDrive} />
            <Metric label="Deployment mode" value="local" detail="remote VPS deferred" icon={HardDrive} />
          </MetricsGrid>

          {servers.length > 0 ? (
            <div className="grid gap-4">
              {servers.map((server) => (
                <Panel key={server.id}>
                  <div className="flex flex-wrap items-start justify-between gap-4">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <h2 className="truncate text-lg font-semibold">{server.name}</h2>
                        <StatusPill status={server.status} />
                        <span className="pill bg-surface-alt text-muted ring-line">{server.kind}</span>
                      </div>
                      <p className="muted mt-1">{server.kind === "local" ? "Default deploy target on this machine" : server.publicIp || "No IP saved"}</p>
                    </div>
                    <div className="text-right text-sm">
                      <div className="eyebrow">Last seen</div>
                      <div className="mt-1 font-medium">{formatLastSeen(server.lastSeenAt)}</div>
                    </div>
                  </div>
                </Panel>
              ))}
            </div>
          ) : (
            <EmptyState
              icon={Server}
              title={error || emptyMessage}
              description="The local agent should appear here when Hostlet is running."
            />
          )}
    </AppShell>
  );
}
