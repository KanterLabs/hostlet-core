"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";

type Server = { id: string; name: string; publicIp?: string; kind: string; status: string; lastSeenAt?: string };

export default function Servers() {
  const [servers, setServers] = useState<Server[]>([]);
  const [message, setMessage] = useState("Loading machines...");

  useEffect(() => {
    api<Server[]>("/api/servers")
      .then((rows) => {
        setServers(rows);
        setMessage(rows.length ? "" : "No machines yet.");
      })
      .catch((e) => setMessage(`Could not load machines. ${e instanceof Error ? e.message : "Sign in again."}`));
  }, []);

  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <div className="mb-6 flex items-center justify-between">
          <h1 className="text-2xl font-semibold">Servers</h1>
          <Link className="button" href="/servers/new">Add VPS</Link>
        </div>
        <div className="space-y-3">
          {servers.map((s) => (
            <div key={s.id} className="rounded-lg border border-line bg-white p-4">
              <div className="flex items-center justify-between">
                <div><div className="font-medium">{s.name}</div><p className="muted">{s.kind === "local" ? "Default deploy target" : s.publicIp || "No IP saved"}</p></div>
                <span className={`rounded-full px-2 py-1 text-xs ${s.status === "online" ? "bg-emerald-100 text-emerald-800" : "bg-neutral-100 text-neutral-700"}`}>{s.status}</span>
              </div>
            </div>
          ))}
          {message && (
            <div className="rounded-lg border border-line bg-white p-6">
              <p className="text-sm text-neutral-700">{message}</p>
              <Link className="button mt-4" href="/servers/new">Add VPS</Link>
            </div>
          )}
        </div>
      </section>
    </main>
  );
}
