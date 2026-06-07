import { expect, test, type Page } from "@playwright/test";

test("app detail shows normalized resource metrics", async ({ page }) => {
  await mockAppDetailApi(page);
  await page.goto("/apps/app-1");

  await expect(page.getByRole("heading", { name: "Resource usage" })).toBeVisible();
  await expect(page.getByText("12.5 MB")).toBeVisible();
  await expect(page.getByText("1.0 GB limit · 1.22%")).toBeVisible();
  await expect(page.getByText("1.2 KB RX")).toBeVisible();
  await expect(page.getByText("0 B TX")).toBeVisible();
  await expect(page.getByText("3.8 MB read")).toBeVisible();
  await expect(page.getByText("976.6 KB written")).toBeVisible();
  await expect(page.locator(".metric").filter({ hasText: "Processes" }).filter({ hasText: "7" })).toBeVisible();
});

async function mockAppDetailApi(page: Page) {
  await page.route("**/*", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/session" || path === "/api/setup/status") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ authenticated: true, mode: "self_hosted", cloud: null, unlocked: true }),
      });
    }
    if (path === "/api/apps/app-1") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: "app-1",
          name: "Metrics App",
          repoFullName: "hostlet-ci/metrics-app",
          branch: "main",
          domain: "metrics.example.test",
          containerPort: 3000,
          healthPath: "/health",
          runtimeKind: "single",
          packagingStrategy: "generated",
          rootDirectory: ".",
          currentDeploymentId: "deploy-1",
          publicExposure: true,
          autoDeploy: false,
          server: { id: "server-1", name: "local", kind: "local", status: "online" },
          latestDeployment: { id: "deploy-1", status: "success", commitSha: "abcdef123456" },
          currentDeployment: { status: "success", publishedPort: 32123 },
          health: { status: "healthy", failureCount: 0, successCount: 3 },
        }),
      });
    }
    if (path === "/api/apps/app-1/resources") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          container: "hostlet-app-app-1",
          cpuPercent: "0.00%",
          cpuPercentValue: 0,
          memoryUsage: "12.5MiB / 1GiB",
          memoryUsageBytes: 13_107_200,
          memoryLimitBytes: 1_073_741_824,
          memoryPercent: "1.22%",
          memoryPercentValue: 1.22,
          networkIo: "1.2kB / 0B",
          networkRxBytes: 1_200,
          networkTxBytes: 0,
          blockIo: "4.0MB / 1.0MB",
          blockReadBytes: 4_000_000,
          blockWriteBytes: 1_000_000,
          pids: "7",
          pidsCurrent: 7,
          sampledAt: new Date().toISOString(),
        }),
      });
    }
    if (path === "/api/apps/app-1/health") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ status: "healthy", failureCount: 0, successCount: 3 }),
      });
    }
    if (path === "/api/apps/app-1/health/events" || path === "/api/apps/app-1/env") {
      return route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
    }
    if (path === "/api/apps/app-1/screenshots/latest") {
      return route.fulfill({ status: 404, body: "not found" });
    }
    if (path.startsWith("/api") || path.startsWith("/auth")) {
      return route.fulfill({ status: 404, body: "not mocked" });
    }
    return route.continue();
  });
}
