export type ResourceStats = {
  cpuPercent: string;
  cpuPercentValue?: number | null;
  memoryUsage: string;
  memoryUsageBytes?: number | null;
  memoryLimitBytes?: number | null;
  memoryPercent: string;
  memoryPercentValue?: number | null;
  networkIo: string;
  networkRxBytes?: number | null;
  networkTxBytes?: number | null;
  blockIo: string;
  blockReadBytes?: number | null;
  blockWriteBytes?: number | null;
  pids: string;
  pidsCurrent?: number | null;
  sampledAt: string;
};

export type RuntimeHealth = {
  appId?: string;
  deploymentId?: string | null;
  containerName?: string | null;
  status: string;
  checkedUrl?: string | null;
  httpStatus?: number | null;
  latencyMs?: number | null;
  failureCount: number;
  successCount: number;
  lastError?: string | null;
  lastCheckedAt?: string | null;
  lastHealthyAt?: string | null;
  updatedAt?: string | null;
};

export type RuntimeHealthEvent = {
  id: string;
  status: string;
  httpStatus?: number | null;
  latencyMs?: number | null;
  error?: string | null;
  createdAt: string;
};

export type AppScreenshot = {
  id: string;
  appId: string;
  deploymentId?: string | null;
  source: string;
  contentType: string;
  byteSize: number;
  width?: number | null;
  height?: number | null;
  capturedAt: string;
  publicUrl: string;
};

export type RuntimeMetadata = {
  runtime?: string | null;
  packagingStrategy?: string | null;
  buildBackend?: string | null;
  generatedDockerfile?: boolean | null;
  detectedFramework?: string | null;
  webService?: string | null;
  runtimeKind?: string | null;
  packageManager?: string | null;
  gitSyncDurationMs?: number | null;
  buildPlanDurationMs?: number | null;
  buildDurationMs?: number | null;
  imageSizeBytes?: number | null;
  composeUpDurationMs?: number | null;
  containerStartDurationMs?: number | null;
  healthCheckDurationMs?: number | null;
  bootDurationMs?: number | null;
  routingDurationMs?: number | null;
};

export type App = {
  id: string;
  name: string;
  repoFullName: string;
  branch: string;
  domain: string;
  containerPort?: number | null;
  healthPath?: string | null;
  runtimeKind?: string | null;
  hostletConfigPath?: string | null;
  packagingStrategy?: string | null;
  rootDirectory?: string | null;
  installCommand?: string | null;
  buildCommand?: string | null;
  startCommand?: string | null;
  memoryLimitMb?: number | null;
  cpuLimit?: number | null;
  currentDeploymentId?: string | null;
  publicExposure?: boolean | null;
  autoDeploy?: boolean | null;
  server?: { id: string; name: string; kind: string; status: string; publicIp?: string | null; lastSeenAt?: string | null } | null;
  latestDeployment?: { id: string; status?: string | null; failure?: string | null; commitSha?: string | null; startedAt?: string | null; finishedAt?: string | null; runtimeMetadata?: RuntimeMetadata | null } | null;
  currentDeployment?: { status: string; publishedPort?: number | null; finishedAt?: string | null } | null;
  latestWebhook?: {
    status: string;
    ignoredReason?: string | null;
    commitSha?: string | null;
    branch?: string | null;
    createdAt?: string | null;
  } | null;
  health?: RuntimeHealth | null;
};

export type AgentJob = {
  id: string;
  status: "queued" | "running" | "success" | "failed";
  failure?: string | null;
};

export type SettingsForm = {
  domain: string;
  health_path: string;
  runtime_kind: string;
  hostlet_config_path: string;
  packaging_strategy: string;
  root_directory: string;
  install_command: string;
  build_command: string;
  start_command: string;
  container_port: string;
  memory_limit_mb: string;
  cpu_limit: string;
  public_exposure: boolean;
  auto_deploy: boolean;
};

export const emptySettings: SettingsForm = {
  domain: "",
  health_path: "/",
  runtime_kind: "single",
  hostlet_config_path: "hostlet.yml",
  packaging_strategy: "auto",
  root_directory: ".",
  install_command: "",
  build_command: "",
  start_command: "",
  container_port: "3000",
  memory_limit_mb: "",
  cpu_limit: "",
  public_exposure: false,
  auto_deploy: false,
};

/**
 * Discrete set of in-flight operations on the app detail page. The empty
 * string is the idle state; a non-empty value gates every button (truthiness)
 * and discriminates which button shows its busy label. Kept as a named type so
 * the gate and the per-button label checks stay in sync.
 */
export type BusyAction =
  | "deploy"
  | "rollback"
  | "exposure"
  | "delete"
  | "settings"
  | "env"
  | "health"
  | "restart"
  | "screenshot"
  | "";
