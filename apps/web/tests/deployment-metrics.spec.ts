import { expect, test, type Page } from "@playwright/test";

const validRuntimeMetadata = {
  packagingStrategy: "generated",
  buildBackend: "railpack",
  detectedFramework: "Node",
  packageManager: "npm",
  gitSyncDurationMs: 450,
  buildPlanDurationMs: 75,
  buildDurationMs: 12500,
  imageSizeBytes: 0,
  containerStartDurationMs: 0,
  healthCheckDurationMs: 2000,
  bootDurationMs: 2000,
  routingDurationMs: 85,
};

test("deployment detail shows startup and boot metrics", async ({ page }) => {
  await mockDeploymentApi(page);
  await page.goto("/deployments/deploy-1");

  await expect(page.getByRole("heading", { name: "Deployment logs" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Deployment metrics" })).toBeVisible();
  await expect(page.getByText("Git sync")).toBeVisible();
  await expect(page.getByText("Build plan")).toBeVisible();
  await expect(page.getByText("75 ms")).toBeVisible();
  await expect(page.getByText("Container start")).toBeVisible();
  await expect(page.getByText("0 ms", { exact: true })).toBeVisible();
  await expect(page.getByText("0 B")).toBeVisible();
  await expect(page.getByText("Health wait")).toBeVisible();
  await expect(page.getByText("Boot time")).toBeVisible();
  await expect(page.locator(".data-label").filter({ hasText: "Routing" })).toBeVisible();
  await expect(page.getByText("2s", { exact: true })).toHaveCount(2);
});

test("deployment detail tolerates malformed runtime metrics", async ({ page }) => {
  await mockDeploymentApi(page, {
    ...validRuntimeMetadata,
    buildPlanDurationMs: "stalled",
    buildDurationMs: "slow",
    imageSizeBytes: "large",
    containerStartDurationMs: -1,
    healthCheckDurationMs: Number.NaN,
  });
  await page.goto("/deployments/deploy-1");

  await expect(page.getByRole("heading", { name: "Deployment metrics" })).toBeVisible();
  await expect(page.getByText("n/a")).toHaveCount(5);
  await expect(page.getByText("NaN")).toHaveCount(0);
});

async function mockDeploymentApi(page: Page, runtimeMetadata: Record<string, unknown> = validRuntimeMetadata) {
  await page.route("**/*", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/session" || path === "/api/setup/status") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ authenticated: true, mode: "self_hosted", cloud: null, unlocked: true }),
      });
    }
    if (path === "/api/deployments/deploy-1") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: "deploy-1",
          appId: "app-1",
          status: "success",
          commitSha: "abc1234",
          failure: null,
          runtimeMetadata,
        }),
      });
    }
    if (path === "/api/deployments/deploy-1/logs") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([{ stream: "stdout", line: "Health check passed." }]),
      });
    }
    if (path.startsWith("/api") || path.startsWith("/auth")) {
      return route.fulfill({ status: 404, body: "not mocked" });
    }
    return route.continue();
  });
}
