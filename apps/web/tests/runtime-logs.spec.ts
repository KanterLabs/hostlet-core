import { expect, test, type Page } from "@playwright/test";
import { jsonRoute, mockApi, textRoute } from "./support/mockApi";

const app = {
  id: "app-1",
  name: "Runtime App",
  repoFullName: "hostlet-ci/runtime-app",
  branch: "main",
  domain: "runtime.example.test",
  runtimeKind: "compose",
  currentDeploymentId: "deployment-1",
  publicExposure: true,
  autoDeploy: false,
  server: { id: "server-1", name: "local", kind: "local", status: "online" },
  latestDeployment: { id: "deployment-1", status: "success", commitSha: "abcdef123456" },
};

type AppFixture = Omit<typeof app, "currentDeploymentId" | "latestDeployment"> & {
  currentDeploymentId: string | null;
  latestDeployment: typeof app.latestDeployment | null;
};

async function mockApp(
  page: Page,
  runtimeHandler: Parameters<typeof mockApi>[1],
  appFixture: AppFixture = app,
) {
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1") {
      await jsonRoute(route, appFixture);
      return true;
    }
    if (
      path === "/api/apps/app-1/env"
      || path === "/api/apps/app-1/health/events"
    ) {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1/health") {
      await jsonRoute(route, { status: "healthy", failureCount: 0, successCount: 1 });
      return true;
    }
    if (path === "/api/apps/app-1/runtime-logs") {
      return (await runtimeHandler?.(route, path)) || false;
    }
    return false;
  });
}

test("shows, polls, and manually refreshes per-service runtime output", async ({ page }) => {
  let requests = 0;
  await mockApp(page, async (route) => {
    requests += 1;
    await jsonRoute(route, {
      deploymentId: "deployment-1",
      capturedAt: "2026-07-28T12:35:00Z",
      truncated: true,
      lines: [
        {
          timestamp: "2026-07-28T12:34:56Z",
          service: "web",
          stream: "stdout",
          line: `request ${requests}`,
        },
        {
          timestamp: "2026-07-28T12:34:57Z",
          service: "postgres",
          stream: "stderr",
          line: "checkpoint complete",
        },
      ],
      unavailableServices: [],
    });
    return true;
  });
  await page.goto("/apps/app-1");

  const panel = page.locator("section.panel", { hasText: "Runtime logs" });
  await expect(panel.getByText("[web]")).toBeVisible();
  await expect(panel.getByText("[postgres]")).toBeVisible();
  await expect(panel.getByText("request 1")).toBeVisible();
  await expect(panel.getByText("500-line, 256 KiB limit")).toBeVisible();
  expect(requests).toBe(1);

  await expect.poll(() => requests, { timeout: 12_000 }).toBeGreaterThanOrEqual(2);
  await expect(panel.getByText(`request ${requests}`)).toBeVisible();
  const beforeRefresh = requests;
  await panel.getByRole("button", { name: "Refresh" }).click();
  await expect.poll(() => requests).toBeGreaterThan(beforeRefresh);
  await expect(panel.getByText(`request ${requests}`)).toBeVisible();
});

test("shows an explicit unavailable state and supports retry", async ({ page }) => {
  await mockApp(page, async (route) => {
    await textRoute(route, "The app's agent is offline. Runtime logs are temporarily unavailable.", 503);
    return true;
  });
  await page.goto("/apps/app-1");

  const panel = page.locator("section.panel", { hasText: "Runtime logs" });
  await expect(panel.getByText("Runtime logs unavailable.")).toBeVisible();
  await expect(panel.getByText("The app's agent is offline.")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Try again" })).toBeVisible();
});

test("does not request runtime logs before the first deployment", async ({ page }) => {
  let requests = 0;
  await mockApp(
    page,
    async () => {
      requests += 1;
      return true;
    },
    {
      ...app,
      currentDeploymentId: null,
      latestDeployment: null,
    },
  );

  await page.goto("/apps/app-1");

  const panel = page.locator("section.panel", { hasText: "Runtime logs" });
  await expect(panel.getByText("Deploy this app once")).toBeVisible();
  await page.waitForTimeout(500);
  expect(requests).toBe(0);
});

test("keeps available output when one service is unavailable", async ({ page }) => {
  await mockApp(page, async (route) => {
    await jsonRoute(route, {
      deploymentId: "deployment-1",
      capturedAt: "2026-07-28T12:35:00Z",
      truncated: false,
      lines: [{
        timestamp: "2026-07-28T12:34:56Z",
        service: "web",
        stream: "stdout",
        line: "web remains visible",
      }],
      unavailableServices: [{
        service: "worker",
        message: "Runtime logs are unavailable for this service.",
      }],
    });
    return true;
  });

  await page.goto("/apps/app-1");

  const panel = page.locator("section.panel", { hasText: "Runtime logs" });
  await expect(panel.getByText("Logs were unavailable for worker.")).toBeVisible();
  await expect(panel.getByText("web remains visible")).toBeVisible();
  await expect(panel.getByRole("region", { name: "Runtime log output" })).toBeVisible();
});
