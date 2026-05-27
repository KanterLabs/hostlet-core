"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { AlertTriangle, Box, CheckCircle2, CreditCard, GitBranch, HardDrive, Lock, Plus, Search, Server, WandSparkles } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, DataList, Field, Notice, PageHeader, Panel, SectionHeader, SelectField, StatusPill, SummaryItem, ToggleCard } from "@/components/ui";
import { WebhookNotice } from "@/components/WebhookNotice";

type Repo = { full_name: string; private: boolean; default_branch: string; updated_at?: string };
type ServerRow = { id: string; name: string; kind: string; status: string };
type CloudflareStatus = {
  baseDomain?: string | null;
  defaultDomainPattern?: string | null;
};
type SessionPayload = {
  mode: "self_hosted" | "cloud";
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
  } | null;
};
type InspectEnv = { key: string; required?: boolean; value?: string; source?: string };
type RepoInspection = {
  repoFullName: string;
  defaultBranch: string;
  branch: string;
  appName: string;
  deployable: boolean;
  runtimeKind: string;
  rootDirectory: string;
  containerPort: number;
  healthPath: string;
  hostletConfigPath: string;
  runtimeConfig: Record<string, unknown>;
  env: InspectEnv[];
  warnings: string[];
  summary: string;
};
type CreateAppForm = {
  name: string;
  repo_full_name: string;
  branch: string;
  server_id: string;
  container_port: number;
  health_path: string;
  domain: string;
  runtime_kind: string;
  hostlet_config_path: string;
  root_directory: string;
  install_command: string;
  build_command: string;
  start_command: string;
  memory_limit_mb: number;
  cpu_limit: number;
  public_exposure: boolean;
  auto_deploy: boolean;
  runtime_config: Record<string, unknown>;
};

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
    runtime_kind: "single",
    hostlet_config_path: "hostlet.yml",
    root_directory: ".",
    install_command: "",
    build_command: "",
    start_command: "",
    memory_limit_mb: 512,
    cpu_limit: 1,
    public_exposure: false,
    auto_deploy: false,
    runtime_config: {} as Record<string, unknown>,
  });
  const [inspection, setInspection] = useState<RepoInspection | null>(null);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [inspecting, setInspecting] = useState(false);
  const [repoLink, setRepoLink] = useState("");
  const [repos, setRepos] = useState<Repo[]>([]);
  const [repoSearch, setRepoSearch] = useState("");
  const [repoMessage, setRepoMessage] = useState("Loading GitHub repositories...");
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [cloudflare, setCloudflare] = useState<CloudflareStatus | null>(null);
  const [session, setSession] = useState<SessionPayload | null>(null);
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
    api<CloudflareStatus>("/api/cloudflare/status")
      .then(setCloudflare)
      .catch(() => setCloudflare(null));
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
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
    setInspection(null);
    setEnvValues({});
  }

  function selectRepo(repo: Repo) {
    setRepoLink(`https://github.com/${repo.full_name}`);
    setForm((current) => ({
      ...current,
      repo_full_name: repo.full_name,
      branch: repo.default_branch || current.branch,
      name: current.name || repo.full_name.split("/")[1].replace(/[^a-zA-Z0-9-]/g, "-").toLowerCase(),
    }));
    setInspection(null);
    setEnvValues({});
  }

  async function inspectRepo() {
    if (!form.repo_full_name || inspecting) return;
    setInspecting(true);
    setMessage("Inspecting repository...");
    try {
      const result = await api<RepoInspection>("/api/github/repo-inspect", {
        method: "POST",
        body: JSON.stringify({ repo_full_name: form.repo_full_name, branch: form.branch }),
      });
      setInspection(result);
      setEnvValues(Object.fromEntries((result.env || []).map((item) => [item.key, item.value || ""])));
      setForm((current) => ({
        ...current,
        name: current.name || result.appName,
        branch: result.branch || current.branch,
        runtime_kind: result.runtimeKind || current.runtime_kind,
        root_directory: result.rootDirectory || current.root_directory,
        container_port: result.containerPort || current.container_port,
        health_path: result.healthPath || current.health_path,
        hostlet_config_path: result.hostletConfigPath || current.hostlet_config_path,
        runtime_config: result.runtimeConfig || {},
      }));
      setMessage(result.deployable ? "Review the inferred runtime, then create and deploy." : "Hostlet could not infer a deployable runtime.");
    } catch (error) {
      setMessage(`Inspect failed. ${error instanceof Error ? error.message : "Check the public GitHub URL."}`);
    } finally {
      setInspecting(false);
    }
  }

  async function submit() {
    if (creating) return;
    setCreating(true);
    setMessage("Creating app...");
    try {
      const env = inspection ? (inspection.env || []).filter((item) => envValues[item.key]).map((item) => ({ key: item.key, value: envValues[item.key] })) : [];
      const payload: Record<string, unknown> = {
        ...form,
        server_id: form.server_id || null,
        env,
        deploy_after_create: !!inspection?.deployable,
      };
      if (cloud) {
        delete payload.memory_limit_mb;
        delete payload.cpu_limit;
      }
      const res = await api<{ id: string; deploymentId?: string | null }>("/api/apps", {
        method: "POST",
        body: JSON.stringify(payload),
      });
      setMessage(inspection?.deployable ? "App created. Opening deployment logs..." : "App created. Opening deploy screen...");
      router.push(res.deploymentId ? `/deployments/${res.deploymentId}` : `/apps/${res.id}`);
    } catch (error) {
      setMessage(`Create failed. ${error instanceof Error ? error.message : "Check the repo, server, port, and domain."}`);
      setCreating(false);
    }
  }

  const cloud = session?.mode === "cloud";
  const cloudReady = !cloud || session?.cloud?.nextStep === "ready";
  const selectedServer = servers.find((server) => server.id === form.server_id);
  const generatedDomain = useMemo(() => {
    if (!cloudflare?.baseDomain) return "";
    const source = form.name || form.repo_full_name.split("/")[1] || "app";
    return `${slugAppName(source)}.${cloudflare.baseDomain}`;
  }, [cloudflare?.baseDomain, form.name, form.repo_full_name]);
  const routePreview = form.domain.trim() || generatedDomain || "Hostlet will generate one";
  const requiredEnvMissing = inspection?.env?.some((item) => item.required && !envValues[item.key]?.trim()) || false;
  const createDisabledReason = createAppDisabledReason({
    cloud,
    cloudReady,
    session,
    form,
    requiredEnvMissing,
    inspection,
  });
  const canCreate = !createDisabledReason;

  return (
    <AppShell>
          <PageHeader
            eyebrow="New application"
            title="Create app"
            description={cloud ? "Choose a GitHub repo and deploy it to an always-on Hostlet Cloud URL." : "Choose a GitHub repo, local runtime settings, and optional automation."}
            actions={<Link className="button-secondary" href="/apps"><Box size={16} />Back to apps</Link>}
          />

          {!cloud && <WebhookNotice autoDeployEnabled={form.auto_deploy} className="mb-6" />}
          {cloud && !cloudReady && (
            <Notice
              tone="warning"
              className="mb-6"
              description={session?.cloud?.githubInstalled ? "Choose a plan before creating cloud apps." : "Install the Hostlet GitHub App before creating cloud apps."}
              action={
                session?.cloud?.githubInstalled ? (
                  <Link href="/" className="button"><CreditCard size={16} />Open billing setup</Link>
                ) : (
                  <a href="/auth/github/install/start" className="button"><GitBranch size={16} />Install GitHub App</a>
                )
              }
            />
          )}

          <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_360px]">
            <div className="space-y-6">
              <Panel>
                <SectionHeader
                  icon={GitBranch}
                  title="Repository"
                  action={form.repo_full_name && <StatusPill status="success" label={form.repo_full_name} />}
                />
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
                <button className="button-secondary mt-4" type="button" disabled={!form.repo_full_name || inspecting} onClick={inspectRepo}>
                  <WandSparkles size={16} />
                  {inspecting ? "Inspecting..." : "Inspect repo"}
                </button>
                {inspection && (
                  <div className="mt-4 rounded-md border border-line bg-surface-alt p-4">
                    <div className="flex items-center gap-2 text-sm font-medium text-ink">
                      {inspection.deployable ? <CheckCircle2 size={16} className="text-action" /> : <AlertTriangle size={16} className="text-red-700" />}
                      {inspection.summary}
                    </div>
                    {inspection.warnings.length > 0 && (
                      <div className="mt-3 space-y-2">
                        {inspection.warnings.map((warning) => (
                          <p key={warning} className="text-sm text-muted">{warning}</p>
                        ))}
                      </div>
                    )}
                    {inspection.env.length > 0 && (
                      <div className="mt-4 grid gap-3">
                        {inspection.env.map((item) => (
                          <Field
                            key={item.key}
                            label={`${item.key}${item.required ? " required" : ""}`}
                            value={envValues[item.key] || ""}
                            onChange={(value) => setEnvValues((current) => ({ ...current, [item.key]: value }))}
                            placeholder={item.source || "Environment value"}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </Panel>

              <Panel>
                <SectionHeader icon={Server} title={cloud ? "Cloud route" : "Local target and route"} />
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="App name" value={form.name} onChange={(value) => setForm({ ...form, name: value })} placeholder="my-app" />
                  <Field label="Branch" value={form.branch} onChange={(value) => setForm({ ...form, branch: value })} placeholder="main" />
                  {!cloud && (
                    <SelectField label="Deploy target" value={form.server_id} onChange={(value) => setForm({ ...form, server_id: value })}>
                      {servers.map((server) => <option key={server.id} value={server.id}>{server.name} (local)</option>)}
                      {servers.length === 0 && <option value="">This machine</option>}
                    </SelectField>
                  )}
                  {!cloud && (
                    <div>
                      <Field
                        label="Domain"
                        value={form.domain}
                        onChange={(value) => setForm({ ...form, domain: value })}
                        placeholder={generatedDomain || cloudflare?.defaultDomainPattern || "optional for local deploys"}
                      />
                      {generatedDomain && <p className="muted mt-2 text-sm">Default route: {generatedDomain}</p>}
                    </div>
                  )}
                </div>
                {cloud ? (
                  <Notice tone="neutral" className="mt-4" description={`Hostlet will use ${generatedDomain || cloudflare?.defaultDomainPattern || "a hostlet.cloud URL"} when available, then add a short suffix only if needed.`} />
                ) : (
                  <div className="mt-4 grid gap-3 sm:grid-cols-2">
                    <ToggleCard checked={form.public_exposure} onChange={(value) => setForm({ ...form, public_exposure: value })} icon={Lock} label="Publish app URL" />
                    <ToggleCard checked={form.auto_deploy} onChange={(value) => setForm({ ...form, auto_deploy: value })} icon={GitBranch} label="Auto redeploy" />
                  </div>
                )}
              </Panel>

              <Panel>
                <SectionHeader icon={Box} title="Runtime" />
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="Root directory" value={form.root_directory} onChange={(value) => setForm({ ...form, root_directory: value })} placeholder="." />
                  <Field label="Container port" type="number" value={String(form.container_port)} onChange={(value) => setForm({ ...form, container_port: Number(value) })} />
                  <Field label="Health path" value={form.health_path} onChange={(value) => setForm({ ...form, health_path: value })} />
                  <SelectField label="Runtime" value={form.runtime_kind} onChange={(value) => setForm({ ...form, runtime_kind: value })}>
                    <option value="single">Dockerfile or Node</option>
                    {!cloud && <option value="compose">Docker Compose</option>}
                  </SelectField>
                  {form.runtime_kind === "compose" && <Field label="Hostlet config" value={form.hostlet_config_path} onChange={(value) => setForm({ ...form, hostlet_config_path: value })} placeholder="hostlet.yml" />}
                  {!cloud && (
                    <>
                      <SelectField label="Memory limit" value={form.memory_limit_mb} onChange={(value) => setForm({ ...form, memory_limit_mb: Number(value) })}>
                        <option value={256}>256 MB</option>
                        <option value={512}>512 MB</option>
                        <option value={1024}>1 GB</option>
                        <option value={2048}>2 GB</option>
                        <option value={4096}>4 GB</option>
                      </SelectField>
                      <SelectField label="CPU limit" value={form.cpu_limit} onChange={(value) => setForm({ ...form, cpu_limit: Number(value) })}>
                        <option value={0.25}>0.25 CPU</option>
                        <option value={0.5}>0.5 CPU</option>
                        <option value={1}>1 CPU</option>
                        <option value={2}>2 CPUs</option>
                        <option value={4}>4 CPUs</option>
                      </SelectField>
                    </>
                  )}
                </div>
                <div className="mt-4 grid gap-4">
                  <Field label="Install command" value={form.install_command} onChange={(value) => setForm({ ...form, install_command: value })} placeholder="auto, npm install, pnpm install" />
                  <Field label="Build command" value={form.build_command} onChange={(value) => setForm({ ...form, build_command: value })} placeholder="optional, npm run build" />
                  <Field label="Start command" value={form.start_command} onChange={(value) => setForm({ ...form, start_command: value })} placeholder="npm start, vite preview --host 0.0.0.0 --port $PORT" />
                </div>
              </Panel>
            </div>

            <aside className="space-y-6 xl:sticky xl:top-7 xl:self-start">
              <Panel>
                <SectionHeader title="Create summary" />
                <DataList className="mt-4">
                  <SummaryItem label="Repo" value={form.repo_full_name || "Choose a repo"} />
                  <SummaryItem label={cloud ? "Worker" : "Machine"} value={cloud ? "Hostlet Cloud managed worker" : selectedServer ? `${selectedServer.name} · local · ${selectedServer.status}` : "This machine"} />
                  <SummaryItem label="Route" value={cloud ? `${routePreview} if available` : routePreview} />
                  <SummaryItem label="Runtime" value={`${form.runtime_kind === "compose" ? "Compose" : "Single"} · :${form.container_port}${form.health_path}`} />
                  <SummaryItem label="Automation" value={cloud ? "manual deploy · public Hostlet URL" : `${form.auto_deploy ? "auto deploy" : "manual deploy"} · ${form.public_exposure ? "public" : "private"}`} />
                </DataList>
                <button className="mt-4 w-full" disabled={creating || !canCreate} onClick={submit}>
                  <Plus size={16} />
                  {creating ? "Creating..." : inspection?.deployable ? "Create and deploy" : "Create app"}
                </button>
                {createDisabledReason && <p className="muted mt-2 text-sm">{createDisabledReason}</p>}
                {message && <Notice tone={message.toLowerCase().includes("failed") ? "danger" : "neutral"} className="mt-3" description={message} />}
              </Panel>
              <Notice
                tone="neutral"
                description={
                  <div>
                    <div className="flex items-center gap-2 font-medium text-ink">
                      <HardDrive size={17} />
                      Target status
                    </div>
                    <p className="muted mt-2">{cloud ? "Hostlet Cloud will build from GitHub, start a managed container, health check it, then publish the Hostlet Cloud URL after success." : "Hostlet will build from GitHub, start a Docker container on this machine, health check it, then publish the route after success."}</p>
                  </div>
                }
              />
            </aside>
          </div>
    </AppShell>
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

function slugAppName(value: string) {
  const slug = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "app";
}

function createAppDisabledReason({
  cloud,
  cloudReady,
  session,
  form,
  requiredEnvMissing,
  inspection,
}: {
  cloud: boolean;
  cloudReady: boolean;
  session: SessionPayload | null;
  form: CreateAppForm;
  requiredEnvMissing: boolean;
  inspection: RepoInspection | null;
}) {
  if (cloud && !session?.cloud?.githubInstalled) return "Install the Hostlet GitHub App before creating cloud apps.";
  if (cloud && !session?.cloud?.billingActive) return "Choose a Hostlet Cloud plan before creating cloud apps.";
  if (cloud && !cloudReady) return "Finish Hostlet Cloud setup before creating apps.";
  if (!form.repo_full_name) return "Choose a GitHub repository.";
  if (!form.name.trim()) return "Enter an app name.";
  if (!form.branch.trim()) return "Enter a branch.";
  if (!cloud && !form.server_id) return "Choose a local deploy target.";
  if (requiredEnvMissing) return "Fill required environment values from the repo inspection.";
  if (inspection?.deployable === false) return "This repo is not deployable yet. Add a Dockerfile or package.json, then inspect again.";
  return "";
}
