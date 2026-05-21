"use client";

import { useEffect, useState } from "react";
import { HardDrive, Server } from "lucide-react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";
import { EmptyState, Metric, PageHeader, StatusPill } from "@/components/ui";

type ServerRow = { id: string; name: string; publicIp?: string; kind: string; status: string; lastSeenAt?: string };

export default function Servers() {
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [message, setMessage] = useState("Loading machines...");

  useEffect(() => {
    api<ServerRow[]>("/api/servers")
      .then((rows) => {
        setServers(rows);
        setMessage(rows.length ? "" : "No machines yet.");
      })
      .catch((error) => setMessage(`Could not load machines. ${error instanceof Error ? error.message : "Sign in again."}`));
  }, []);

  const online = servers.filter((server) => server.status === "online").length;
  const local = servers.filter((server) => server.kind === "local").length;

  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
          <PageHeader
            eyebrow="Machines"
            title="This machine"
            description="Hostlet 0.1.0 deploys apps onto the same machine that runs this control plane."
          />

          <div className="mb-6 grid gap-4 md:grid-cols-3">
            <Metric label="Local machines" value={String(local)} detail="current deploy target" icon={Server} />
            <Metric label="Online agents" value={`${online}/${servers.length || 1}`} detail="websocket heartbeat" icon={HardDrive} />
            <Metric label="Deployment mode" value="local" detail="remote VPS deferred" icon={HardDrive} />
          </div>

          {servers.length > 0 ? (
            <div className="grid gap-4">
              {servers.map((server) => (
                <article key={server.id} className="panel p-4">
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
                      <div className="mt-1 font-medium">{server.lastSeenAt ? new Date(server.lastSeenAt).toLocaleString() : "Not seen yet"}</div>
                    </div>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <EmptyState
              icon={Server}
              title={message}
              description="The local agent should appear here when Hostlet is running."
            />
          )}
        </div>
      </section>
    </main>
  );
}
