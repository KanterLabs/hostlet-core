"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Nav } from "@/components/Nav";
import { api } from "@/lib/api";

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
  });
  const [repoLink, setRepoLink] = useState("");
  const [repos, setRepos] = useState<Array<{ full_name: string; private: boolean; default_branch: string; updated_at?: string }>>([]);
  const [repoSearch, setRepoSearch] = useState("");
  const [repoMessage, setRepoMessage] = useState("Loading GitHub repos...");
  const [servers, setServers] = useState<Array<{ id: string; name: string; kind: string; status: string }>>([]);
  const [message, setMessage] = useState("");
  const [createdAppId, setCreatedAppId] = useState("");
  const [creating, setCreating] = useState(false);
  useEffect(() => {
    api<Array<{ id: string; name: string; kind: string; status: string }>>("/api/servers")
      .then((rows) => {
        setServers(rows);
        const local = rows.find((s) => s.kind === "local");
        if (local) setForm((f) => ({ ...f, server_id: local.id }));
      })
      .catch(() => {});
    api<Array<{ full_name: string; private: boolean; default_branch: string; updated_at?: string }>>("/api/github/repos")
      .then((rows) => {
        setRepos(rows);
        setRepoMessage(rows.length ? "" : "No repositories returned from GitHub.");
      })
      .catch((e) => setRepoMessage(`Could not load repos. ${e instanceof Error ? e.message : "Paste a repo link instead."}`));
  }, []);

  function updateRepoLink(value: string) {
    setRepoLink(value);
    const repo = parseGitHubRepo(value);
    if (!repo) {
      setForm((f) => ({ ...f, repo_full_name: "" }));
      return;
    }
    setForm((f) => ({
      ...f,
      repo_full_name: repo,
      name: f.name || repo.split("/")[1].replace(/[^a-zA-Z0-9-]/g, "-").toLowerCase(),
    }));
  }

  async function submit() {
    if (creating) return;
    setCreating(true);
    setMessage("Creating app...");
    try {
      const res = await api<{ id: string }>("/api/apps", { method: "POST", body: JSON.stringify({ ...form, server_id: form.server_id || null, env: [] }) });
      setCreatedAppId(res.id);
      setMessage("App created. Opening app...");
      router.push(`/apps/${res.id}`);
    } catch (e) {
      setMessage(`Create failed. Check the repo, server id, port, and domain. ${e instanceof Error ? e.message : ""}`);
      setCreating(false);
    }
  }
  function selectRepo(repo: { full_name: string; default_branch: string }) {
    setRepoLink(`https://github.com/${repo.full_name}`);
    setForm((f) => ({
      ...f,
      repo_full_name: repo.full_name,
      branch: repo.default_branch || f.branch,
      name: f.name || repo.full_name.split("/")[1].replace(/[^a-zA-Z0-9-]/g, "-").toLowerCase(),
    }));
  }
  const filteredRepos = repos.filter((repo) => repo.full_name.toLowerCase().includes(repoSearch.toLowerCase())).slice(0, 100);
  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="max-w-2xl p-8">
        <h1 className="text-2xl font-semibold">Create app</h1>
        <div className="mt-6 grid gap-4">
          <div className="rounded-lg border border-line bg-white p-4">
            <div className="mb-3 flex items-center justify-between gap-3">
              <h2 className="text-sm font-medium">Choose a GitHub repo</h2>
              {form.repo_full_name && <span className="rounded-full bg-emerald-50 px-2 py-1 text-xs text-emerald-800">{form.repo_full_name}</span>}
            </div>
            <input value={repoSearch} onChange={(e) => setRepoSearch(e.target.value)} placeholder="Search repositories" />
            {repoMessage && <p className="muted mt-3">{repoMessage}</p>}
            {filteredRepos.length > 0 && (
              <div className="mt-3 max-h-72 overflow-y-auto rounded-md border border-line">
                {filteredRepos.map((repo) => (
                  <button
                    key={repo.full_name}
                    type="button"
                    onClick={() => selectRepo(repo)}
                    className={`flex w-full items-center justify-between rounded-none border-b border-line bg-white px-3 py-2 text-left text-ink hover:bg-panel ${form.repo_full_name === repo.full_name ? "bg-emerald-50" : ""}`}
                  >
                    <span>
                      <span className="block text-sm font-medium">{repo.full_name}</span>
                      <span className="text-xs text-neutral-600">{repo.private ? "Private" : "Public"} · {repo.default_branch}</span>
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
          <label className="text-sm font-medium">GitHub repo link
            <input
              value={repoLink}
              onChange={(e) => updateRepoLink(e.target.value)}
              placeholder="https://github.com/owner/repo"
            />
          </label>
          {form.repo_full_name && <p className="muted">Repo: {form.repo_full_name}</p>}
          {repoLink && !form.repo_full_name && <p className="text-sm text-red-700">Paste a GitHub URL, SSH URL, or owner/repo.</p>}
          <label className="text-sm font-medium">app name
            <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="my-app" />
          </label>
          <label className="text-sm font-medium">branch
            <input value={form.branch} onChange={(e) => setForm({ ...form, branch: e.target.value })} placeholder="main" />
          </label>
          <label className="text-sm font-medium">deploy target
            <select value={form.server_id} onChange={(e) => setForm({ ...form, server_id: e.target.value })}>
              {servers.map((server) => <option key={server.id} value={server.id}>{server.name}{server.kind === "local" ? " (default)" : ""}</option>)}
              {servers.length === 0 && <option value="">This machine</option>}
            </select>
          </label>
          <div className="grid gap-4 rounded-lg border border-line bg-white p-4">
            <h2 className="text-sm font-medium">Runtime</h2>
            <label className="text-sm font-medium">root directory
              <input value={form.root_directory} onChange={(e) => setForm({ ...form, root_directory: e.target.value })} placeholder="." />
            </label>
            <label className="text-sm font-medium">install command
              <input value={form.install_command} onChange={(e) => setForm({ ...form, install_command: e.target.value })} placeholder="auto, npm install, pnpm install" />
            </label>
            <label className="text-sm font-medium">build command
              <input value={form.build_command} onChange={(e) => setForm({ ...form, build_command: e.target.value })} placeholder="optional, npm run build" />
            </label>
            <label className="text-sm font-medium">start command
              <input value={form.start_command} onChange={(e) => setForm({ ...form, start_command: e.target.value })} placeholder="npm start, vite preview --host 0.0.0.0 --port $PORT" />
            </label>
            <label className="text-sm font-medium">container port
              <input value={form.container_port} type="number" onChange={(e) => setForm({ ...form, container_port: Number(e.target.value) })} />
            </label>
            <label className="text-sm font-medium">health path
              <input value={form.health_path} onChange={(e) => setForm({ ...form, health_path: e.target.value })} />
            </label>
            <label className="text-sm font-medium">domain
              <input value={form.domain} onChange={(e) => setForm({ ...form, domain: e.target.value })} placeholder="optional for local deploys" />
            </label>
          </div>
          <div className="grid gap-4 rounded-lg border border-line bg-white p-4">
            <div>
              <h2 className="text-sm font-medium">Resource limits</h2>
              <p className="muted mt-1">These limits apply to each running app container.</p>
            </div>
            <label className="text-sm font-medium">memory limit
              <select value={form.memory_limit_mb} onChange={(e) => setForm({ ...form, memory_limit_mb: Number(e.target.value) })}>
                <option value={256}>256 MB</option>
                <option value={512}>512 MB</option>
                <option value={1024}>1 GB</option>
                <option value={2048}>2 GB</option>
                <option value={4096}>4 GB</option>
              </select>
            </label>
            <label className="text-sm font-medium">CPU limit
              <select value={form.cpu_limit} onChange={(e) => setForm({ ...form, cpu_limit: Number(e.target.value) })}>
                <option value={0.25}>0.25 CPU</option>
                <option value={0.5}>0.5 CPU</option>
                <option value={1}>1 CPU</option>
                <option value={2}>2 CPUs</option>
                <option value={4}>4 CPUs</option>
              </select>
            </label>
          </div>
          <button disabled={creating || !form.repo_full_name || !form.name || !form.branch} onClick={submit}>
            {creating ? "Creating..." : "Create app"}
          </button>
          {message && (
            <div className="rounded-md border border-line bg-panel p-3 text-sm">
              <p>{message}</p>
              {createdAppId && <Link className="button mt-3" href={`/apps/${createdAppId}`}>Open app</Link>}
            </div>
          )}
        </div>
      </section>
    </main>
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
