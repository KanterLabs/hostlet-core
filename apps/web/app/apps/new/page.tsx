"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { AlertTriangle, Box, CheckCircle2, GitBranch, HardDrive, Lock, Plus, Search, Server, WandSparkles } from "lucide-react";
import { api } from "@/lib/api";
import { AppShell, DataList, Field, Notice, PageHeader, Panel, SectionHeader, SelectField, StatusPill, SummaryItem, ToggleCard } from "@/components/ui";
import { WebhookNotice } from "@/components/WebhookNotice";
import {
  CreateAppForm,
  RepoInspection,
  createAppDisabledReason,
  defaultCreateAppForm,
  envValuesFromInspection,
  mergeInspectionIntoForm,
  parseGitHubRepo,
  slugAppName,
} from "./createAppForm";

type Repo = { full_name: string; private: boolean; default_branch: string; updated_at?: string };
type ServerRow = { id: string; name: string; kind: string; status: string };
type CloudflareStatus = {
  baseDomain?: string | null;
  defaultDomainPattern?: string | null;
};

export default function CreateApp() {
  const router = useRouter();
  const [form, setForm] = useState<CreateAppForm>(defaultCreateAppForm);
  // Typed, single-field updater: avoids repeated `{ ...form, x }` spreads in JSX
  // and uses the functional updater form to sidestep stale-closure bugs.
  const setField = useCallback(
    <K extends keyof CreateAppForm>(key: K, value: CreateAppForm[K]) =>
      setForm((current) => ({ ...current, [key]: value })),
    [],
  );
  const [inspection, setInspection] = useState<RepoInspection | null>(null);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [inspecting, setInspecting] = useState(false);
  const [repoLink, setRepoLink] = useState("");
  const [repos, setRepos] = useState<Repo[]>([]);
  const [repoSearch, setRepoSearch] = useState("");
  const [repoMessage, setRepoMessage] = useState("Loading GitHub repositories...");
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [cloudflare, setCloudflare] = useState<CloudflareStatus | null>(null);
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
      setEnvValues(envValuesFromInspection(result));
      setForm((current) => mergeInspectionIntoForm(current, result));
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
        install_command: null,
        env,
        deploy_after_create: !!inspection?.deployable,
      };
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

  const selectedServer = servers.find((server) => server.id === form.server_id);
  const generatedDomain = useMemo(() => {
    if (!cloudflare?.baseDomain) return "";
    const source = form.name || form.repo_full_name.split("/")[1] || "app";
    return `${slugAppName(source)}.${cloudflare.baseDomain}`;
  }, [cloudflare?.baseDomain, form.name, form.repo_full_name]);
  const routePreview = form.domain.trim() || generatedDomain || "Hostlet will generate one";
  const requiredEnvMissing = inspection?.env?.some((item) => item.required && !envValues[item.key]?.trim()) || false;
  const createDisabledReason = createAppDisabledReason({ form, requiredEnvMissing, inspection });
  const canCreate = !createDisabledReason;

  return (
    <AppShell>
          <PageHeader
            eyebrow="New application"
            title="Create app"
            description="Choose a GitHub repo, local runtime settings, and optional automation."
            actions={<Link className="button-secondary" href="/apps"><Box size={16} />Back to apps</Link>}
          />

          <WebhookNotice autoDeployEnabled={form.auto_deploy} className="mb-6" />

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
                    {(inspection.detectedFramework || inspection.packageManager) && (
                      <div className="mt-3 grid gap-2 sm:grid-cols-2">
                        <SummaryItem label="Framework" value={inspection.detectedFramework || "Custom Dockerfile"} />
                        <SummaryItem label="Package manager" value={inspection.packageManager || "n/a"} />
                      </div>
                    )}
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
                <SectionHeader icon={Server} title="Local target and route" />
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="App name" value={form.name} onChange={(value) => setField("name", value)} placeholder="my-app" />
                  <Field label="Branch" value={form.branch} onChange={(value) => setField("branch", value)} placeholder="main" />
                  <SelectField label="Deploy target" value={form.server_id} onChange={(value) => setField("server_id", value)}>
                    {servers.map((server) => <option key={server.id} value={server.id}>{server.name} (local)</option>)}
                    {servers.length === 0 && <option value="">This machine</option>}
                  </SelectField>
                  <div>
                    <Field
                      label="Domain"
                      value={form.domain}
                      onChange={(value) => setField("domain", value)}
                      placeholder={generatedDomain || cloudflare?.defaultDomainPattern || "optional for local deploys"}
                    />
                    {generatedDomain && <p className="muted mt-2 text-sm">Default route: {generatedDomain}</p>}
                  </div>
                </div>
                <div className="mt-4 grid gap-3 sm:grid-cols-2">
                  <ToggleCard checked={form.public_exposure} onChange={(value) => setField("public_exposure", value)} icon={Lock} label="Publish app URL" />
                  <ToggleCard checked={form.auto_deploy} onChange={(value) => setField("auto_deploy", value)} icon={GitBranch} label="Auto redeploy" />
                </div>
              </Panel>

              <Panel>
                <SectionHeader icon={Box} title="Runtime" />
                <div className="grid gap-4 md:grid-cols-2">
                  <Field label="Root directory" value={form.root_directory} onChange={(value) => setField("root_directory", value)} placeholder="." />
                  <Field label="Container port" type="number" value={String(form.container_port)} onChange={(value) => setField("container_port", Number(value))} />
                  <Field label="Health path" value={form.health_path} onChange={(value) => setField("health_path", value)} />
                  <SelectField label="Runtime" value={form.runtime_kind} onChange={(value) => setField("runtime_kind", value)}>
                    <option value="single">Single service app</option>
                    <option value="compose">Docker Compose</option>
                  </SelectField>
                  <SelectField label="Package with" value={form.packaging_strategy} onChange={(value) => setField("packaging_strategy", value)}>
                    <option value="auto">Auto detect</option>
                    <option value="dockerfile">Repository Dockerfile</option>
                    <option value="generated">Railpack generated runtime</option>
                  </SelectField>
                  {form.runtime_kind === "compose" && <Field label="Hostlet config" value={form.hostlet_config_path} onChange={(value) => setField("hostlet_config_path", value)} placeholder="hostlet.yml" />}
                  <SelectField label="Memory limit" value={form.memory_limit_mb} onChange={(value) => setField("memory_limit_mb", Number(value))}>
                    <option value={256}>256 MB</option>
                    <option value={512}>512 MB</option>
                    <option value={1024}>1 GB</option>
                    <option value={2048}>2 GB</option>
                    <option value={4096}>4 GB</option>
                  </SelectField>
                  <SelectField label="CPU limit" value={form.cpu_limit} onChange={(value) => setField("cpu_limit", Number(value))}>
                    <option value={0.25}>0.25 CPU</option>
                    <option value={0.5}>0.5 CPU</option>
                    <option value={1}>1 CPU</option>
                    <option value={2}>2 CPUs</option>
                    <option value={4}>4 CPUs</option>
                  </SelectField>
                </div>
                <div className="mt-4 grid gap-4">
                  <Field label="Build command" value={form.build_command} onChange={(value) => setField("build_command", value)} placeholder="optional Railpack override, npm run build, go build, cargo build --release" />
                  <Field label="Start command" value={form.start_command} onChange={(value) => setField("start_command", value)} placeholder="optional Railpack override, npm start, uvicorn main:app --host 0.0.0.0 --port $PORT" />
                </div>
              </Panel>
            </div>

            <aside className="space-y-6 xl:sticky xl:top-7 xl:self-start">
              <Panel>
                <SectionHeader title="Create summary" />
                <DataList className="mt-4">
                  <SummaryItem label="Repo" value={form.repo_full_name || "Choose a repo"} />
                  <SummaryItem label="Machine" value={selectedServer ? `${selectedServer.name} · local · ${selectedServer.status}` : "This machine"} />
                  <SummaryItem label="Route" value={routePreview} />
                  <SummaryItem label="Runtime" value={`${form.runtime_kind === "compose" ? "Compose" : "Single"} · :${form.container_port}${form.health_path}`} />
                  <SummaryItem label="Automation" value={`${form.auto_deploy ? "auto deploy" : "manual deploy"} · ${form.public_exposure ? "public" : "private"}`} />
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
                    <p className="muted mt-2">Hostlet will build from GitHub, start a Docker container on this machine, health check it, then publish the route after success.</p>
                  </div>
                }
              />
            </aside>
          </div>
    </AppShell>
  );
}
