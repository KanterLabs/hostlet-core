"use client";

import { useState } from "react";
import Link from "next/link";
import { CheckCircle2, Clipboard, HardDrive, Plus, Server } from "lucide-react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";
import { Field, PageHeader } from "@/components/ui";

export default function AddServer() {
  const [name, setName] = useState("Production VPS");
  const [publicIp, setPublicIp] = useState("");
  const [token, setToken] = useState("");
  const [installCommand, setInstallCommand] = useState("");
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [creating, setCreating] = useState(false);

  async function submit() {
    if (creating) return;
    setError("");
    setCreating(true);
    try {
      const res = await api<{ id: string; installToken: string; installCommand: string }>("/api/servers", {
        method: "POST",
        body: JSON.stringify({ name, public_ip: publicIp || null }),
      });
      setToken(res.installToken);
      setInstallCommand(res.installCommand);
    } catch (error) {
      setError(`Could not create server. ${error instanceof Error ? error.message : "Try signing in again."}`);
    } finally {
      setCreating(false);
    }
  }

  async function copyCommand() {
    await navigator.clipboard.writeText(installCommand);
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  }

  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner max-w-5xl">
          <PageHeader
            eyebrow="Remote agent"
            title="Add VPS"
            description="Create a one-time install token, then run the generated command on the server that should run app containers."
            actions={<Link className="button-secondary" href="/servers"><HardDrive size={16} />Back to servers</Link>}
          />

          <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_360px]">
            <section className="panel p-4">
              <div className="mb-4 flex items-center gap-2">
                <Server size={18} />
                <h2 className="font-semibold">Server details</h2>
              </div>
              <div className="grid gap-4">
                <Field label="Server name" value={name} onChange={setName} placeholder="Production VPS" />
                <Field label="Public IP" value={publicIp} onChange={setPublicIp} placeholder="optional" />
              </div>
              <button className="mt-4" disabled={creating || !name.trim()} onClick={submit}>
                <Plus size={16} />
                {creating ? "Creating..." : "Create install token"}
              </button>
              {error && <p className="mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800">{error}</p>}
            </section>

            <aside className="panel-muted p-4">
              <div className="font-semibold">What the command does</div>
              <p className="muted mt-2">It installs Docker and Caddy if missing, builds the Hostlet agent, registers it, and enables a systemd service.</p>
            </aside>
          </div>

          {token && (
            <section className="panel mt-6 p-4">
              <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
                <div className="flex items-center gap-2">
                  <CheckCircle2 size={18} className="text-action" />
                  <h2 className="font-semibold">Install command ready</h2>
                </div>
                <button className="button-secondary" onClick={copyCommand}><Clipboard size={16} />{copied ? "Copied" : "Copy"}</button>
              </div>
              <pre className="code-box">{installCommand}</pre>
              <Link className="button mt-4" href="/servers">Back to machines</Link>
            </section>
          )}
        </div>
      </section>
    </main>
  );
}
