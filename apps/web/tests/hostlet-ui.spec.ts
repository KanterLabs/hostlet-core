import { expect, test } from "@playwright/test";
import { mockApi } from "./support/mockApi";

test("self-hosted overview and create flow stay available", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
  await expect(page.getByText("Docker + Caddy")).toBeVisible();

  await page.goto("/apps/new");
  await expect(page.getByRole("heading", { name: "Create app" })).toBeVisible();
  await expect(page.getByText("Local target and route")).toBeVisible();
  await expect(page.getByText("Runtime", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Container port")).toHaveCount(0);
  await expect(page.getByLabel("Build command")).toHaveCount(0);
  await expect(page.getByLabel("Start command")).toHaveCount(0);
});

test("create flow submits inferred runtime defaults without runtime controls", async ({ page }) => {
  let createPayload: Record<string, unknown> | null = null;
  await mockApi(page, async (route, path) => {
    if (path === "/api/github/repo-inspect") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          repoFullName: "hostlet-ci/generated-app",
          defaultBranch: "main",
          branch: "main",
          appName: "generated-app",
          deployable: true,
          runtimeKind: "single",
          rootDirectory: "apps/web",
          containerPort: 4321,
          healthPath: "/ready",
          hostletConfigPath: "hostlet.yml",
          runtimeConfig: {},
          packagingStrategy: "auto",
          recommendedPackagingStrategy: "generated",
          detectedFramework: "Next.js",
          packageManager: "pnpm",
          env: [],
          warnings: [],
          summary: "Next.js app detected.",
        }),
      });
      return true;
    }
    if (path === "/api/apps" && route.request().method() === "POST") {
      createPayload = route.request().postDataJSON();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ id: "app-1", deploymentId: "deploy-1" }),
      });
      return true;
    }
    return false;
  });

  await page.goto("/apps/new");
  await page.getByLabel("GitHub repo link").fill("https://github.com/hostlet-ci/generated-app");
  await page.getByRole("button", { name: "Inspect repo" }).click();
  await expect(page.getByText("Auto generated")).toBeVisible();
  await expect(page.getByLabel("Container port")).toHaveCount(0);

  await page.getByRole("button", { name: "Create and deploy" }).click();

  expect(createPayload).toMatchObject({
    repo_full_name: "hostlet-ci/generated-app",
    runtime_kind: "single",
    root_directory: "apps/web",
    container_port: 4321,
    health_path: "/ready",
    packaging_strategy: "generated",
    deploy_after_create: true,
  });
});

test("settings show self-hosted provider status", async ({ page }) => {
  await mockApi(page);
  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByText("GitHub Device Flow is configured.")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Cloudflare" })).toBeVisible();
});
