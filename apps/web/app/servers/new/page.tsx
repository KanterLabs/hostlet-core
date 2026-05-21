"use client";

import { useState } from "react";
import Link from "next/link";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";

export default function AddServer() {
  const [name, setName] = useState("Production VPS");
  const [publicIp, setPublicIp] = useState("");
  const [token, setToken] = useState("");
  const [installCommand, setInstallCommand] = useState("");
  const [error, setError] = useState("");
  const [creating, setCreating] = useState(false);
  async function submit() {
    if (creating) return;
    setError("");
    setCreating(true);
    try {
      const res = await api<{ id: string; installToken: string; installCommand: string }>("/api/servers", { method: "POST", body: JSON.stringify({ name, public_ip: publicIp || null }) });
      setToken(res.installToken);
      setInstallCommand(res.installCommand);
    } catch (e) {
      setError(`Could not create server. ${e instanceof Error ? e.message : "Try signing in again."}`);
    } finally {
      setCreating(false);
    }
  }
  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="max-w-2xl p-8">
        <h1 className="text-2xl font-semibold">Add VPS</h1>
        <p className="muted mt-2">This machine is already available. Add a VPS when you want to deploy somewhere else.</p>
        <div className="mt-6 space-y-4">
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Server name" />
          <input value={publicIp} onChange={(e) => setPublicIp(e.target.value)} placeholder="Public IP" />
          <button disabled={creating || !name.trim()} onClick={submit}>{creating ? "Creating..." : "Create install token"}</button>
          {error && <p className="rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800">{error}</p>}
          {token && (
            <div className="rounded-lg border border-line bg-white p-4">
              <p className="text-sm font-medium">Run this on the VPS</p>
              <pre className="mt-3 overflow-x-auto rounded-md border border-line bg-panel p-4 text-sm">{installCommand}</pre>
              <Link className="button mt-4" href="/servers">Back to machines</Link>
            </div>
          )}
        </div>
      </section>
    </main>
  );
}
