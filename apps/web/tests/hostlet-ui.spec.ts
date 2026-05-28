import { expect, test, type Page } from "@playwright/test";

const now = new Date("2026-05-27T00:00:00Z").toISOString();

test("cloud setup gates create actions until GitHub App and billing are ready", async ({ page }) => {
  await mockApi(page, { mode: "cloud", githubInstalled: false, billingActive: false });
  await page.goto("/");

  await expect(page.getByText("Finish Hostlet Cloud setup")).toBeVisible();
  await expect(page.getByRole("link", { name: /Install GitHub App/i }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: /Create app/i }).first()).toBeDisabled();

  await page.goto("/apps/new");
  await expect(page.getByText("Install the Hostlet GitHub App before creating cloud apps.").first()).toBeVisible();
  await expect(page.getByRole("button", { name: /Create app/i })).toBeDisabled();
});

test("cloud app forms show managed 0.4.1 automation without editable controls", async ({ page }) => {
  await mockApi(page, { mode: "cloud", githubInstalled: true, billingActive: true });

  await page.goto("/apps/new");
  await expect(page.getByText("Plan resources")).toHaveCount(0);
  await expect(page.getByText(/CPU limit/i)).toHaveCount(0);
  await expect(page.getByText(/Memory limit/i)).toHaveCount(0);
  await expect(page.getByLabel("Runtime")).not.toContainText("Docker Compose");
  await expect(page.getByText("Publish app URL")).toHaveCount(0);
  await expect(page.getByText("Auto redeploy on push")).toBeVisible();

  await page.goto("/apps/smoke-app");
  await expect(page.getByText("Managed cloud settings")).toBeVisible();
  await expect(page.getByText("Managed auto redeploy")).toBeVisible();
  await expect(page.getByText("managed on push")).toBeVisible();
  await expect(page.locator(".eyebrow", { hasText: "Hostlet Cloud URL" }).first()).toBeVisible();
  await expect(page.getByText("Public URL")).toHaveCount(0);
  await expect(page.getByText("Auto redeploy on branch push")).toHaveCount(0);
});

test("cloud overview, settings, and pricing show app limits", async ({ page }) => {
  await mockApi(page, { mode: "cloud", githubInstalled: true, billingActive: true });

  await page.goto("/");
  await expect(page.getByText("Plan usage")).toBeVisible();
  await expect(page.getByText("1/2")).toBeVisible();
  await expect(page.getByText("1 slots remaining")).toBeVisible();

  await page.goto("/settings");
  await expect(page.getByText("Starter plan")).toBeVisible();
  await expect(page.getByText("2 apps")).toBeVisible();
  await expect(page.getByRole("button", { name: /Manage subscription/i })).toBeVisible();

  await page.goto("/pricing");
  await expect(page.getByRole("heading", { name: "Hostlet Cloud plans" })).toBeVisible();
  await expect(page.getByText("1 app included")).toBeVisible();
  await expect(page.getByText("2 apps included")).toBeVisible();
  await expect(page.getByText("4 apps included")).toBeVisible();
});

test("self-hosted app forms expose local controls", async ({ page }) => {
  await mockApi(page, { mode: "self_hosted", githubInstalled: false, billingActive: false });

  await page.goto("/apps/new");
  await expect(page.getByText("Publish app URL")).toBeVisible();
  await expect(page.getByText("Auto redeploy")).toBeVisible();
  await expect(page.getByLabel("Runtime")).toContainText("Docker Compose");

  await page.goto("/apps/smoke-app");
  await expect(page.getByText("Automation")).toBeVisible();
  await expect(page.locator(".eyebrow", { hasText: "Public URL" })).toBeVisible();
  await expect(page.getByText("Auto redeploy on branch push")).toBeVisible();
});

test("deployment logs remain readable after success", async ({ page }) => {
  await mockApi(page, { mode: "self_hosted", githubInstalled: false, billingActive: false });
  await page.goto("/deployments/smoke-deployment");

  await expect(page.getByRole("heading", { name: "Deployment logs" })).toBeVisible();
  await expect(page.getByText("Deployment succeeded. Logs remain available here.")).toBeVisible();
  await expect(page.locator("pre")).toContainText("stdout: Health check passed.");
});

test("runtime security headers are present", async ({ request }) => {
  const response = await request.get("/");
  await expect(response).toBeOK();
  expect(response.headers()["x-frame-options"]).toBe("DENY");
  expect(response.headers()["x-content-type-options"]).toBe("nosniff");
  expect(response.headers()["content-security-policy"]).toContain("frame-ancestors 'none'");
  expect(response.headers()["referrer-policy"]).toBe("same-origin");
  expect(response.headers()["permissions-policy"]).toContain("camera=()");
});

type MockOptions = {
  mode: "self_hosted" | "cloud";
  githubInstalled: boolean;
  billingActive: boolean;
};

async function mockApi(page: Page, options: MockOptions) {
  const cloud = options.mode === "cloud";
  const app = {
    id: "smoke-app",
    name: "Smoke App",
    repoFullName: "hostlet-ci/node-hello",
    branch: "main",
    domain: cloud ? "smoke.hostlet.cloud" : "localhost:23000",
    currentDeploymentId: "smoke-deployment",
    runtimeKind: "single",
    hostletConfigPath: "hostlet.yml",
    runtimeConfig: {},
    rootDirectory: ".",
    installCommand: null,
    buildCommand: null,
    startCommand: null,
    containerPort: 3000,
    healthPath: "/health",
    memoryLimitMb: cloud ? 512 : 1024,
    cpuLimit: cloud ? 0.5 : 1,
    publicExposure: true,
    autoDeploy: true,
    server: {
      id: "00000000-0000-0000-0000-000000000001",
      name: "This machine",
      kind: "local",
      status: "online",
      publicIp: "127.0.0.1",
    },
    latestDeployment: {
      id: "smoke-deployment",
      status: "success",
      commitSha: "1234567890abcdef",
      failure: null,
      startedAt: now,
      finishedAt: now,
    },
    currentDeployment: {
      status: "success",
      publishedPort: 32001,
      finishedAt: now,
    },
    latestWebhook: null,
    health: {
      status: "healthy",
      httpStatus: 200,
      latencyMs: 24,
      failureCount: 0,
      successCount: 4,
      lastError: null,
      lastCheckedAt: now,
      lastHealthyAt: now,
      containerName: "hostlet-app-smoke",
      checkedUrl: "http://127.0.0.1:32001/health",
    },
  };

  await page.route(/.*\/api\/.*/, async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path === "/api/setup/status") return json({ mode: options.mode, setupRequired: false, unlocked: true });
    if (path === "/api/session") {
      return json({
        mode: options.mode,
        authenticated: true,
        user: { id: "ci-user", login: "ci-user" },
        cloud: cloud
          ? {
              billingActive: options.billingActive,
              githubInstalled: options.githubInstalled,
              nextStep: options.githubInstalled ? (options.billingActive ? "ready" : "billing") : "install_github",
              planCode: options.billingActive ? "starter" : null,
              subscriptionStatus: options.billingActive ? "active" : null,
            }
          : null,
      });
    }
    if (path === "/api/cloud/usage") {
      return json({
        planCode: options.billingActive ? "starter" : null,
        subscriptionStatus: options.billingActive ? "active" : null,
        currentPeriodStart: now,
        currentPeriodEnd: now,
        cancelAtPeriodEnd: false,
        apps: {
          used: options.billingActive ? 1 : 0,
          limit: options.billingActive ? 2 : 0,
          remaining: options.billingActive ? 1 : 0,
        },
      });
    }
    if (path === "/api/cloud/billing/portal") return json({ url: "https://billing.stripe.test/session" });
    if (path === "/api/cloud/billing/checkout") return json({ url: "https://checkout.stripe.test/session" });
    if (path === "/api/github/status") {
      return json({ connected: true, mode: options.mode, cloud: cloud ? { githubInstalled: options.githubInstalled } : null });
    }
    if (path === "/api/github/repos") {
      return json([{ full_name: "hostlet-ci/node-hello", private: false, default_branch: "main" }]);
    }
    if (path === "/api/servers") {
      return json([app.server]);
    }
    if (path === "/api/cloudflare/status") {
      return json({
        configured: !cloud,
        baseDomain: cloud ? "hostlet.cloud" : "example.test",
        defaultDomainPattern: cloud ? "*.hostlet.cloud" : "hostlet-*.example.test",
      });
    }
    if (path === "/api/apps") return json([app]);
    if (path === "/api/apps/smoke-app") return json(app);
    if (path === "/api/apps/smoke-app/env") return json([{ key: "APP_VERSION" }]);
    if (path === "/api/apps/smoke-app/resources") {
      return json({
        cpuPercent: "1.23%",
        memoryUsage: "64MiB / 512MiB",
        memoryPercent: "12.5%",
        networkIo: "1kB / 2kB",
        blockIo: "0B / 0B",
        pids: "12",
        sampledAt: now,
      });
    }
    if (path === "/api/apps/smoke-app/health") return json(app.health);
    if (path === "/api/apps/smoke-app/health/events") return json([]);
    if (path === "/api/deployments/smoke-deployment") {
      return json({ id: "smoke-deployment", appId: "smoke-app", status: "success", commitSha: "1234567890abcdef", failure: null });
    }
    if (path === "/api/deployments/smoke-deployment/logs") {
      return json([
        { stream: "stdout", line: "Building image" },
        { stream: "stdout", line: "Health check passed." },
      ]);
    }
    if (path === "/api/system/version") return json({ currentVersion: "0.4.1" });
    if (path === "/api/health/summary") return json({ healthy: 1, degraded: 0, unhealthy: 0, unknown: 0 });
    if (path === "/api/audit-events") return json([]);
    if (path === "/api/agent-jobs") return json([]);
    if (path === "/api/system/backups/latest") return json({ latest: null });
    if (path === "/api/system/cleanup") return json({ database: {}, docker: { keepContainers: 1, keepImages: 1, jobWillRun: !cloud } });
    return json({});
  });
}
