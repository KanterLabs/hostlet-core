"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { HardDrive, Plus, Server, Timer } from "lucide-react";
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
  const remote = servers.filter((server) => server.kind !== "local").length;

  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
          <PageHeader
            eyebrow="Machines"
            title="Servers"
            description="Local and remote agents that can build, run, route, and clean up app containers."
            actions={<Link className="button" href="/servers/new"><Plus size={16} />Add VPS</Link>}
          />

          <div className="mb-6 grid gap-4 md:grid-cols-3">
            <Metric label="Total machines" value={String(servers.length)} detail="local plus VPS targets" icon={Server} />
            <Metric label="Online agents" value={`${online}/${servers.length || 1}`} detail="websocket heartbeat" icon={HardDrive} />
            <Metric label="Remote VPS" value={String(remote)} detail="connected by install token" icon={Timer} />
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
                        <span className="pill bg-neutral-100 text-neutral-700 ring-neutral-200">{server.kind}</span>
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
              description="This machine is available by default. Add a VPS when you want deployments to run somewhere else."
              actionHref="/servers/new"
              actionLabel="Add VPS"
            />
          )}
        </div>
      </section>
    </main>
  );
}
