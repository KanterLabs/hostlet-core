import { expect, test, type Page } from "@playwright/test";
import { jsonRoute, LOCAL_SERVER_ID, mockApi } from "./support/mockApi";

// HCR-004 — create a self-hosted app from a GitHub repo. Browser-proves the
// post-create navigation (router.push to the deployment logs, or the app page
// when the create response carries no deployment) and that the inferred runtime
// override values reach the POST /api/apps payload.
//
// The manual "Runtime" panel (container port / build command / start command /
// memory / CPU) was intentionally removed from this screen by the "Simplify
// self-hosted create flow" change — hostlet-ui.spec.ts asserts those controls
// are absent here. There is no toggle or disclosure that brings them back: the
// runtime overrides are now inferred during repo inspection (container_port,
// health_path, root_directory, packaging_strategy) and otherwise carried at
// their form defaults (build_command/start_command "", memory_limit_mb 512,
// cpu_limit 1). This spec proves every one of those override fields still rides
// along in the captured create request.

const INSPECTION = {
  repoFullName: "acme/orders-api",
  defaultBranch: "main",
  branch: "main",
  appName: "orders-api",
  deployable: true,
  runtimeKind: "single",
  rootDirectory: "apps/api",
  containerPort: 8080,
  healthPath: "/healthz",
  hostletConfigPath: "hostlet.yml",
  runtimeConfig: {},
  packagingStrategy: "auto",
  recommendedPackagingStrategy: "generated",
  detectedFramework: "Express",
  packageManager: "npm",
  env: [],
  warnings: [],
  summary: "Express app detected.",
};

// Stub the inspect call with the runtime-bearing fixture and the create call
// with `createResponse`, capturing the submitted POST body via `onPayload`.
function mockCreateFlow(
  page: Page,
  createResponse: Record<string, unknown>,
  onPayload: (body: Record<string, unknown>) => void,
) {
  return mockApi(page, async (route, path) => {
    if (path === "/api/github/repo-inspect") {
      await jsonRoute(route, INSPECTION);
      return true;
    }
    if (path === "/api/apps" && route.request().method() === "POST") {
      onPayload(route.request().postDataJSON());
      await jsonRoute(route, createResponse);
      return true;
    }
    return false;
  });
}

test("inspect then create opens the deployment logs and posts inferred runtime overrides", async ({ page }) => {
  let payload: Record<string, unknown> | null = null;
  await mockCreateFlow(page, { id: "app-7", deploymentId: "deploy-7" }, (body) => {
    payload = body;
  });
  await page.goto("/apps/new");

  await page.getByLabel("GitHub repo link").fill("https://github.com/acme/orders-api");
  await page.getByRole("button", { name: "Inspect repo" }).click();
  // The Build summary flips to "Auto generated" once a deployable repo is inspected.
  await expect(page.getByText("Auto generated")).toBeVisible();

  await page.getByRole("button", { name: "Create and deploy" }).click();
  await page.waitForURL(/\/deployments\/deploy-7/);

  expect(payload).toMatchObject({
    repo_full_name: "acme/orders-api",
    runtime_kind: "single",
    root_directory: "apps/api",
    container_port: 8080,
    health_path: "/healthz",
    build_command: "",
    start_command: "",
    memory_limit_mb: 512,
    cpu_limit: 1,
    packaging_strategy: "generated",
    deploy_after_create: true,
  });
});

test("create falls back to the app page when the response carries no deployment", async ({ page }) => {
  let payload: Record<string, unknown> | null = null;
  await mockCreateFlow(page, { id: "app-42" }, (body) => {
    payload = body;
  });
  await page.goto("/apps/new");

  await page.getByLabel("GitHub repo link").fill("https://github.com/acme/orders-api");
  await page.getByRole("button", { name: "Inspect repo" }).click();
  await expect(page.getByText("Auto generated")).toBeVisible();

  await page.getByRole("button", { name: "Create and deploy" }).click();
  await page.waitForURL(/\/apps\/app-42/);

  expect(payload).toMatchObject({
    repo_full_name: "acme/orders-api",
    container_port: 8080,
    memory_limit_mb: 512,
    cpu_limit: 1,
    deploy_after_create: true,
  });
});

const topologyCandidate = (name: string, role: "frontend" | "backend", rootDirectory: string) => ({
  selector: `node:${rootDirectory}/package.json:${name}`,
  name,
  role,
  rootDirectory,
  provider: "node",
  packageManager: "pnpm",
  buildCommand: `pnpm --filter ${name}... run build`,
  startCommand: role === "backend" ? `pnpm --filter ${name} run start` : null,
  outputDirectory: role === "frontend" ? `${rootDirectory}/dist` : null,
  containerPort: role === "frontend" ? 80 : 3000,
  healthProbe: role === "frontend" ? { kind: "http", path: "/" } : { kind: "tcp" },
  publicEnv: role === "frontend" ? ["VITE_WS_URL"] : [],
  evidence: [role === "frontend" ? "dependency vite" : "dependency ws"],
});

test("ambiguous topology requires selection and submits the selected services", async ({ page }) => {
  let payload: Record<string, unknown> | null = null;
  const frontend = topologyCandidate("client", "frontend", "packages/client");
  const backend = topologyCandidate("server", "backend", "packages/server");
  await mockApi(page, async (route, path) => {
    if (path === "/api/servers") {
      await jsonRoute(route, [{ id: LOCAL_SERVER_ID, name: "This machine", kind: "local", status: "online", agentProtocolVersion: 3 }]);
      return true;
    }
    if (path === "/api/github/repo-inspect") {
      await jsonRoute(route, {
        ...INSPECTION,
        deployable: false,
        runtimeKind: "single",
        summary: "Hostlet found multiple runnable service candidates.",
        inferencePlan: {
          schemaVersion: 1,
          readiness: "needs_selection",
          confidence: "medium",
          services: [],
          candidates: [frontend, backend],
          routing: null,
          warnings: ["Choose at most one frontend and one backend before deploying."],
          summary: "Hostlet found multiple runnable service candidates.",
        },
      });
      return true;
    }
    if (path === "/api/apps" && route.request().method() === "POST") {
      payload = route.request().postDataJSON();
      await jsonRoute(route, { id: "app-topology", deploymentId: "deploy-topology" });
      return true;
    }
    return false;
  });
  await page.goto("/apps/new");
  await page.getByLabel("GitHub repo link").fill("https://github.com/acme/patchwork");
  await page.getByRole("button", { name: "Inspect repo" }).click();
  await expect(page.getByRole("button", { name: "Create app" })).toBeDisabled();
  await page.getByLabel("Frontend").selectOption(frontend.selector);
  await page.getByLabel("Backend").selectOption(backend.selector);
  await page.getByLabel("Backend path prefixes").fill("/api, /socket.io");
  await page.getByRole("button", { name: "Use selected topology" }).click();
  await page.getByRole("button", { name: "Create and deploy" }).click();
  await page.waitForURL(/\/deployments\/deploy-topology/);
  expect(payload).toMatchObject({
    runtime_kind: "compose",
    runtime_config: { generatedTopology: {
      schemaVersion: 1,
      mode: "selected",
      frontendSelector: frontend.selector,
      backendSelector: backend.selector,
      backendPathPrefixes: ["/api", "/socket.io"],
    } },
  });
});

test("topology deployment is blocked until the agent supports protocol v3", async ({ page }) => {
  const frontend = topologyCandidate("client", "frontend", "packages/client");
  const backend = topologyCandidate("server", "backend", "packages/server");
  await mockApi(page, async (route, path) => {
    if (path === "/api/servers") {
      await jsonRoute(route, [{ id: LOCAL_SERVER_ID, name: "This machine", kind: "local", status: "online", agentProtocolVersion: 2 }]);
      return true;
    }
    if (path === "/api/github/repo-inspect") {
      await jsonRoute(route, { ...INSPECTION, runtimeKind: "compose", runtimeConfig: { generatedTopology: { schemaVersion: 1, mode: "auto" } }, inferencePlan: {
        schemaVersion: 1, readiness: "ready", confidence: "high", services: [frontend, backend], candidates: [frontend, backend],
        routing: { websocketsToBackend: true, backendPathPrefixes: ["/api"] }, warnings: [], summary: "Hostlet found frontend and backend.",
      } });
      return true;
    }
    return false;
  });
  await page.goto("/apps/new");
  await page.getByLabel("GitHub repo link").fill("https://github.com/acme/patchwork");
  await page.getByRole("button", { name: "Inspect repo" }).click();
  await expect(page.getByText(/protocol v3/i)).toBeVisible();
  await expect(page.getByRole("button", { name: "Create and deploy" })).toBeDisabled();
});
