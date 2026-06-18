export type CreateAppForm = {
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
  packaging_strategy: string;
};

/**
 * Default runtime values for a brand-new app form. These mirror the defaults
 * applied when editing an existing app (the `[id]` settings screen), but that
 * screen owns its own copy; keep the values in sync if they change.
 */
export const defaultCreateAppForm: CreateAppForm = {
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
  runtime_config: {},
  packaging_strategy: "auto",
};

export type InspectEnv = { key: string; required?: boolean; value?: string; source?: string };

/** A service detected in a multi-service (Compose) repo, for a read-only preview. */
export type InspectionService = {
  name: string;
  role: string;
  image?: string | null;
  build?: boolean;
  ports?: string[];
  volumes?: string[];
};

export type RepoInspection = {
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
  packagingStrategy?: string;
  packagingOptions?: string[];
  recommendedPackagingStrategy?: string;
  detectedFramework?: string;
  packageManager?: string;
  webService?: string;
  services?: InspectionService[];
  env: InspectEnv[];
  warnings: string[];
  summary: string;
};

/**
 * Merge a repo inspection result onto the current form, preferring the inferred
 * values while falling back to whatever the user already has. Pure: returns the
 * next form without mutating the input.
 */
export function mergeInspectionIntoForm(current: CreateAppForm, result: RepoInspection): CreateAppForm {
  return {
    ...current,
    name: current.name || result.appName,
    branch: result.branch || current.branch,
    runtime_kind: result.runtimeKind || current.runtime_kind,
    root_directory: result.rootDirectory || current.root_directory,
    container_port: result.containerPort || current.container_port,
    health_path: result.healthPath || current.health_path,
    hostlet_config_path: result.hostletConfigPath || current.hostlet_config_path,
    runtime_config: result.runtimeConfig || {},
    packaging_strategy: result.recommendedPackagingStrategy || result.packagingStrategy || current.packaging_strategy,
  };
}

/** Initial env value map seeded from an inspection result. */
export function envValuesFromInspection(result: RepoInspection): Record<string, string> {
  return Object.fromEntries((result.env || []).map((item) => [item.key, item.value || ""]));
}

export function parseGitHubRepo(input: string): string | null {
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

export function slugAppName(value: string) {
  const slug = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "app";
}

export function createAppDisabledReason({
  form,
  requiredEnvMissing,
  inspection,
}: {
  form: CreateAppForm;
  requiredEnvMissing: boolean;
  inspection: RepoInspection | null;
}) {
  if (!form.repo_full_name) return "Choose a GitHub repository.";
  if (!form.name.trim()) return "Enter an app name.";
  if (!form.branch.trim()) return "Enter a branch.";
  if (!form.server_id) return "Choose a local deploy target.";
  if (requiredEnvMissing) return "Fill required environment values from the repo inspection.";
  if (inspection?.deployable === false) return "This repo is not deployable yet. Add a supported app manifest or start command, then inspect again.";
  return "";
}
