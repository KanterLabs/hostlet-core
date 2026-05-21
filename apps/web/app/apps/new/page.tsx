"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import type { LucideIcon } from "lucide-react";
import { Box, CheckCircle2, Cpu, GitBranch, HardDrive, Lock, Plus, Search, Server } from "lucide-react";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";
import { Field, PageHeader, StatusPill } from "@/components/ui";
import { WebhookNotice } from "@/components/WebhookNotice";

type Repo = { full_name: string; private: boolean; default_branch: string; updated_at?: string };
type ServerRow = { id: string; name: string; kind: string; status: string };

export default function CreateApp() {
  const router = useRouter();
  const [form, setForm] = useState({
    name: "",
    repo_full_name: "",
    branch: "main",
    server_id: "",
    container_port: 3000,
    health_path: "/",
    domain: "",
    root_directory: ".",
    install_command: "",
    build_command: "",
    start_command: "",
    memory_limit_mb: 512,
    cpu_limit: 1,
    public_exposure: false,
    auto_deploy: false,
  });
  const [repoLink, setRepoLink] = useState("");
  const [repos, setRepos] = useState<Repo[]>([]);
  const [repoSearch, setRepoSearch] = useState("");
  const [repoMessage, setRepoMessage] = useState("Loading GitHub repositories...");
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [message, setMessage] = useState("");
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    api<ServerRow[]>("/api/servers")
      .then((rows) => {
        setServers(rows);
        const local = rows.find((server) => server.kind === "local");
        if (local) setForm((current) => ({ ...current, server_id: local.id }));
      })
      .catch(() => {});
    api<Repo[]>("/api/github/repos")
      .then((rows) => {
        if (!Array.isArray(rows)) throw new Error("GitHub returned an unexpected repository payload.");
        setRepos(rows);
        setRepoMessage(rows.length ? "" : "No repositories returned from GitHub.");
      })
      .catch((error) => setRepoMessage(`Could not load repos. ${error instanceof Error ? error.message : "Paste a repo link instead."}`));
  }, []);

  const filteredRepos = useMemo(
    () => repos.filter((repo) => repo.full_name.toLowerCase().includes(repoSearch.toLowerCase())).slice(0, 80),
    [repos, repoSearch],
  );

  function updateRepoLink(value: string) {
    setRepoLink(value);
    const repo = parseGitHubRepo(value);
    if (!repo) {
      setForm((current) => ({ ...current, repo_full_name: "" }));
      return;
    }
    setForm((current) => ({
      ...current,
      repo_full_name: repo,
      name: current.name || repo.split("/")[1].replace(/[^a-zA-Z0-9-]/g, "-").toLowerCase(),
    }));
  }

  function selectRepo(repo: Repo) {
    setRepoLink(`https://github.com/${repo.full_name}`);
    setForm((current) => ({
      ...current,
      repo_full_name: repo.full_name,
      branch: repo.default_branch || current.branch,
      name: current.name || repo.full_name.split("/")[1].replace(/[^a-zA-Z0-9-]/g, "-").toLowerCase(),
    }));
  }

  async function submit() {
    if (creating) return;
    setCreating(true);
    setMessage("Creating app...");
    try {
      const res = await api<{ id: string }>("/api/apps", {
        method: "POST",
        body: JSON.stringify({ ...form, server_id: form.server_id || null, env: [] }),
      });
      setMessage("App created. Opening deploy screen...");
      router.push(`/apps/${res.id}`);
    } catch (error) {
      setMessage(`Create failed. ${error instanceof Error ? error.message : "Check the repo, server, port, and domain."}`);
      setCreating(false);
    }
  }

  const selectedServer = servers.find((server) => server.id === form.server_id);
  const canCreate = !!form.repo_full_name && !!form.name && !!form.branch && !!form.server_id;

  return (
    <main className="app-shell">
      <Nav />
      <section className="page">
        <div className="page-inner">
          <PageHeader
            eyebrow="New application"
            title="Create app"
            description="Choose a GitHub repo, target machine, runtime settings, and optional automation."
            actions={<Link className="button-secondary" href="/apps"><Box size={16} />Back to apps</Link>}
          />

          <WebhookNotice autoDeployEnabled={form.auto_deploy} className="mb-6" />

          <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_360px]">
            <div className="space-y-6">
              <section className="panel p-4">
                <div className="mb-4 flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <GitBranch size={18} />
                    <h2 className="font-semibold">Repository</h2>
                  </div>
                  {form.repo_full_name && <StatusPill status="success" label={form.repo_full_name} />}
                </div>
                <label className="block">
                  <span className="flex items-center gap-2"><Search size={15} />Search repositories</span>
                  <input className="mt-1" value={repoSearch} onChange={(event) => setRepoSearch(event.target.value)} placeholder="owner/repo" />
                </label>
                {repoMessage && <p className="muted mt-3">{repoMessage}</p>}
                {filteredRepos.length > 0 && (
                  <div className="mt-3 max-h-80 overflow-y-auto rounded-md border border-line">
                    {filteredRepos.map((repo) => (
                      <button
                        key={repo.full_name}
                        type="button"
                        onClick={() => selectRepo(repo)}
                        className={`flex w-full items-center justify-between rounded-none border-b border-line bg-surface px-3 py-2 text-left text-ink shadow-none last:border-b-0 hover:bg-surface-alt ${
                          form.repo_full_name === repo.full_name ? "bg-emerald-50" : ""
                        }`}
                      >
                        <span className="min-w-0">
                          <span className="block truncate text-sm font-medium">{repo.full_name}</span>
                          <span className="text-xs text-muted">{repo.private ? "Private" : "Public"} · {repo.default_branch}</span>
                        </span>
                        {form.repo_full_name === repo.full_name && <CheckCircle2 size={17} className="text-action" />}
                      </button>
                    ))}
                  </div>
                )}
                <div className="mt-4">
                  <Field label="GitHub repo link" value={repoLink} onChange={updateRepoLink} placeholder="https://github.com/owner/repo" />
                  {repoLink && !form.repo_full_name && <p className="mt-2 text-sm text-red-700">Paste a GitHub URL, SSH URL, or owner/repo.</p>}
                </div>
              </section>

              <section className="panel p-4">
                <div className="mb-4 flex items-center gap-2">
                  <Server size={18} />
                  <h2 className="font-semibold">Target and route</h2>
                </div>
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="App name" value={form.name} onChange={(value) => setForm({ ...form, name: value })} placeholder="my-app" />
                  <Field label="Branch" value={form.branch} onChange={(value) => setForm({ ...form, branch: value })} placeholder="main" />
                  <label className="block">
                    <span>Deploy target</span>
                    <select className="mt-1" value={form.server_id} onChange={(event) => setForm({ ...form, server_id: event.target.value })}>
                      {servers.map((server) => <option key={server.id} value={server.id}>{server.name}{server.kind === "local" ? " (default)" : ""}</option>)}
                      {servers.length === 0 && <option value="">This machine</option>}
                    </select>
                  </label>
                  <Field label="Domain" value={form.domain} onChange={(value) => setForm({ ...form, domain: value })} placeholder="optional for local deploys" />
                </div>
                <div className="mt-4 grid gap-3 sm:grid-cols-2">
                  <Toggle checked={form.public_exposure} onChange={(value) => setForm({ ...form, public_exposure: value })} icon={Lock} label="Publish app URL" />
                  <Toggle checked={form.auto_deploy} onChange={(value) => setForm({ ...form, auto_deploy: value })} icon={GitBranch} label="Auto redeploy" />
                </div>
              </section>

              <section className="panel p-4">
                <div className="mb-4 flex items-center gap-2">
                  <Cpu size={18} />
                  <h2 className="font-semibold">Runtime</h2>
                </div>
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="Root directory" value={form.root_directory} onChange={(value) => setForm({ ...form, root_directory: value })} placeholder="." />
                  <Field label="Container port" type="number" value={String(form.container_port)} onChange={(value) => setForm({ ...form, container_port: Number(value) })} />
                  <Field label="Health path" value={form.health_path} onChange={(value) => setForm({ ...form, health_path: value })} />
                  <label className="block">
                    <span>Memory limit</span>
                    <select className="mt-1" value={form.memory_limit_mb} onChange={(event) => setForm({ ...form, memory_limit_mb: Number(event.target.value) })}>
                      <option value={256}>256 MB</option>
                      <option value={512}>512 MB</option>
                      <option value={1024}>1 GB</option>
                      <option value={2048}>2 GB</option>
                      <option value={4096}>4 GB</option>
                    </select>
                  </label>
                  <label className="block">
                    <span>CPU limit</span>
                    <select className="mt-1" value={form.cpu_limit} onChange={(event) => setForm({ ...form, cpu_limit: Number(event.target.value) })}>
                      <option value={0.25}>0.25 CPU</option>
                      <option value={0.5}>0.5 CPU</option>
                      <option value={1}>1 CPU</option>
                      <option value={2}>2 CPUs</option>
                      <option value={4}>4 CPUs</option>
                    </select>
                  </label>
                </div>
                <div className="mt-4 grid gap-4">
                  <Field label="Install command" value={form.install_command} onChange={(value) => setForm({ ...form, install_command: value })} placeholder="auto, npm install, pnpm install" />
                  <Field label="Build command" value={form.build_command} onChange={(value) => setForm({ ...form, build_command: value })} placeholder="optional, npm run build" />
                  <Field label="Start command" value={form.start_command} onChange={(value) => setForm({ ...form, start_command: value })} placeholder="npm start, vite preview --host 0.0.0.0 --port $PORT" />
                </div>
              </section>
            </div>

            <aside className="space-y-6 xl:sticky xl:top-7 xl:self-start">
              <section className="panel p-4">
                <h2 className="font-semibold">Create summary</h2>
                <div className="mt-4 grid gap-2">
                  <Summary label="Repo" value={form.repo_full_name || "Choose a repo"} />
                  <Summary label="Machine" value={selectedServer ? `${selectedServer.name} · ${selectedServer.status}` : "Choose a target"} />
                  <Summary label="Route" value={form.domain || "Hostlet will generate one"} />
                  <Summary label="Runtime" value={`:${form.container_port}${form.health_path}`} />
                  <Summary label="Automation" value={`${form.auto_deploy ? "auto deploy" : "manual deploy"} · ${form.public_exposure ? "public" : "private"}`} />
                </div>
                <button className="mt-4 w-full" disabled={creating || !canCreate} onClick={submit}>
                  <Plus size={16} />
                  {creating ? "Creating..." : "Create app"}
                </button>
                {message && <p className="mt-3 rounded-md border border-line bg-surface-alt p-3 text-sm text-muted">{message}</p>}
              </section>
              <section className="panel-muted p-4">
                <div className="flex items-center gap-2 font-medium">
                  <HardDrive size={17} />
                  Target status
                </div>
                <p className="muted mt-2">Hostlet will build from GitHub, start a Docker container, health check it, then publish the route after success.</p>
              </section>
            </aside>
          </div>
        </div>
      </section>
    </main>
  );
}

function Toggle({ checked, onChange, icon: Icon, label }: { checked: boolean; onChange: (value: boolean) => void; icon: LucideIcon; label: string }) {
  return (
    <label className={`flex items-center gap-3 rounded-lg border p-3 ${checked ? "border-emerald-200 bg-emerald-50" : "border-line bg-surface"}`}>
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <Icon size={17} className={checked ? "text-action" : "text-neutral-500"} />
      <span>{label}</span>
    </label>
  );
}

function Summary({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md bg-surface-alt px-3 py-2 text-sm">
      <div className="eyebrow">{label}</div>
      <div className="mt-1 break-words font-medium">{value}</div>
    </div>
  );
}

function parseGitHubRepo(input: string): string | null {
  const trimmed = input.trim().replace(/\.git$/, "");
  const shorthand = trimmed.match(/^([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)$/);
  if (shorthand) return `${shorthand[1]}/${shorthand[2]}`;

  const ssh = trimmed.match(/^git@github\.com:([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)$/);
  if (ssh) return `${ssh[1]}/${ssh[2]}`;

  try {
    const url = new URL(trimmed);
    if (url.hostname !== "github.com") return null;
    const [owner, repo] = url.pathname.split("/").filter(Boolean);
    if (!owner || !repo) return null;
    return `${owner}/${repo}`;
  } catch {
    return null;
  }
}
