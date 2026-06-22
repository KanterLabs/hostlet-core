import { expect, test, type Page } from "@playwright/test";
import { jsonRoute, mockApi } from "./support/mockApi";

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
