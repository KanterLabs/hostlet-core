import { expect, test, type Page } from "@playwright/test";
import { firstErrorLine } from "@/components/ui";

const validRuntimeMetadata = {
  packagingStrategy: "generated",
  buildBackend: "railpack",
  detectedFramework: "Node",
  packageManager: "npm",
  gitSyncDurationMs: 450,
  buildPlanDurationMs: 75,
  buildDurationMs: 12500,
  imageSizeBytes: 0,
  imageBudgetStatus: "ok",
  imageBudgetWarnBytes: 500000000,
  imageBudgetMaxBytes: 1000000000,
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
  await expect(page.getByText("Image budget")).toBeVisible();
  await expect(page.getByText("Within budget")).toBeVisible();
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
    imageBudgetStatus: "huge",
    containerStartDurationMs: -1,
    healthCheckDurationMs: Number.NaN,
  });
  await page.goto("/deployments/deploy-1");

  await expect(page.getByRole("heading", { name: "Deployment metrics" })).toBeVisible();
  await expect(page.getByText("n/a")).toHaveCount(6);
  await expect(page.getByText("NaN")).toHaveCount(0);
});

test("deployment detail shows queue position while waiting", async ({ page }) => {
  await mockDeploymentApi(page, validRuntimeMetadata, {
    status: "queued",
    queue: { status: "queued", position: 4, deploysAhead: 3, updatedAt: "2026-06-15T18:00:00Z" },
  });
  await page.goto("/deployments/deploy-1");

  await expect(page.getByText("3 deploys ahead of you")).toBeVisible();
});

test("deployment log first error ignores railpack command flags", async ({ page }) => {
  await mockDeploymentApi(page, validRuntimeMetadata, { status: "failed", failure: "Generated image build failed." }, [
    { stream: "stdout", line: "$ railpack build --name hostlet/app-demo:image --progress plain --error-missing-start /var/lib/hostlet/repos/app-demo" },
    { stream: "stderr", line: "error: no start command could be inferred" },
  ]);
  await page.goto("/deployments/deploy-1");

  await expect(page.getByText("First error in the log")).toBeVisible();
  await expect(page.getByText("stderr: error: no start command could be inferred")).toBeVisible();
});

test("firstErrorLine skips command echoes with error-like flags", () => {
  expect(firstErrorLine([
    "stdout: $ railpack build --name hostlet/app-demo:image --progress plain --error-missing-start /var/lib/hostlet/repos/app-demo",
    "stderr: error: no start command could be inferred",
  ])).toBe("stderr: error: no start command could be inferred");
});

async function mockDeploymentApi(
  page: Page,
  runtimeMetadata: Record<string, unknown> = validRuntimeMetadata,
  deployment: Record<string, unknown> = {},
  logs: Array<{ stream: string; line: string }> = [{ stream: "stdout", line: "Health check passed." }],
) {
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
          ...deployment,
        }),
      });
    }
    if (path === "/api/deployments/deploy-1/logs") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(logs),
      });
    }
    if (path.startsWith("/api") || path.startsWith("/auth")) {
      return route.fulfill({ status: 404, body: "not mocked" });
    }
    return route.continue();
  });
}
