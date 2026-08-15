import { expect, test, type Page } from "@playwright/test";
import { jsonRoute, mockApi } from "./support/mockApi";

// HCR-006 — app detail operations. Browser-proves the action panel's disabled
// states and the disabled-rollback reason text across the three states that
// drive them (active deploy, never-deployed, deployed-idle), which were defined
// only in helper unit logic before. Button queries are scoped to the "App
// actions" panel because the webhook notice also renders a deploy button.

const baseApp = {
  id: "app-1",
  name: "acme-api",
  repoFullName: "acme/api",
  branch: "main",
  domain: "acme-api.example.test",
  rootDirectory: ".",
  runtimeKind: "single",
  autoDeploy: true,
  server: { id: "s", name: "local", kind: "local", status: "online" },
};

// Only /api/apps/app-1 and its /env list need real data; the health/resources/
// screenshot sub-requests fall through to a 404 and are handled gracefully.
function mockAppDetail(page: Page, app: Record<string, unknown>) {
  return mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1") {
      await jsonRoute(route, app);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    return false;
  });
}

const actionsPanel = (page: Page) => page.locator("section.panel", { hasText: "App actions" });

test("disables deploy and explains rollback while a deploy is active", async ({ page }) => {
  await mockAppDetail(page, { ...baseApp, currentDeploymentId: "d0", latestDeployment: { id: "d1", status: "building" } });
  await page.goto("/apps/app-1");
  const actions = actionsPanel(page);

  await expect(actions.getByRole("button", { name: "Deploy latest" })).toBeDisabled();
  const rollback = actions.getByRole("button", { name: "Rollback" });
  await expect(rollback).toBeDisabled();
  await expect(rollback).toHaveAttribute("title", "Wait for the active deployment to finish before rolling back.");
  await expect(page.getByText("Rollback unavailable.")).toBeVisible();
});

test("prompts a first deploy and explains rollback before any deployment", async ({ page }) => {
  await mockAppDetail(page, { ...baseApp, currentDeploymentId: null, latestDeployment: null });
  await page.goto("/apps/app-1");
  const actions = actionsPanel(page);

  await expect(page.getByText("This app has not been deployed yet.")).toBeVisible();
  await expect(actions.getByRole("button", { name: "Rollback" })).toHaveAttribute("title", "Deploy this app once before rolling back.");
  await expect(actions.getByRole("button", { name: "Deploy latest" })).toBeEnabled();
});

test("enables operate + destructive actions for a deployed app", async ({ page }) => {
  await mockAppDetail(page, { ...baseApp, publicExposure: true, currentDeploymentId: "d0", latestDeployment: { id: "d1", status: "success" } });
  await page.goto("/apps/app-1");
  const actions = actionsPanel(page);

  await expect(actions.getByRole("button", { name: "Deploy latest" })).toBeEnabled();
  await expect(actions.getByRole("button", { name: "Rollback" })).toBeEnabled();
  const del = actions.getByRole("button", { name: "Delete" });
  await expect(del).toBeEnabled();
  await expect(del).toHaveClass(/button-danger/);
});

test("runs a browser check and refreshes the app health", async ({ page }) => {
  let appLoads = 0;
  const app = {
    ...baseApp,
    publicExposure: true,
    currentDeploymentId: "d0",
    latestDeployment: { id: "d1", status: "success" },
    health: { status: "healthy", browser: { status: "ready", failure: null } },
  };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1") {
      appLoads += 1;
      await jsonRoute(route, app);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1/browser-check") {
      await jsonRoute(route, { jobId: "job-browser" });
      return true;
    }
    if (path === "/api/agent-jobs/job-browser") {
      await jsonRoute(route, { id: "job-browser", status: "success" });
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");

  await actionsPanel(page).getByRole("button", { name: "Check in browser" }).click();
  await expect(page.getByText("Browser check completed.")).toBeVisible();
  expect(appLoads).toBeGreaterThanOrEqual(2);
});

const persistedSettingsApp = {
  ...baseApp,
  domain: "saved.example.test",
  healthPath: "/health",
  currentDeploymentId: "d0",
  latestDeployment: { id: "d1", status: "success" },
};

test("preserves dirty settings and the warning when publishing", async ({ page }) => {
  let serverApp = { ...persistedSettingsApp, publicExposure: false };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1" && route.request().method() === "PATCH") {
      serverApp = { ...serverApp, publicExposure: true };
      await jsonRoute(route, {});
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");

  await actionsPanel(page).getByRole("button", { name: "Publish URL" }).click();

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByText("Unsaved settings are not included in deploys or auxiliary app actions.")).toBeVisible();
  await expect(page.getByText(/App URL published.*Unsaved settings remain in this form and were not included/)).toBeVisible();
  await expect(page.getByLabel("Public URL")).toBeChecked();
});

test("preserves dirty settings and the warning when unpublishing", async ({ page }) => {
  let serverApp = { ...persistedSettingsApp, publicExposure: true };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1" && route.request().method() === "PATCH") {
      serverApp = { ...serverApp, publicExposure: false };
      await jsonRoute(route, {});
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");

  await actionsPanel(page).getByRole("button", { name: "Make private" }).click();

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByText("Unsaved settings are not included in deploys or auxiliary app actions.")).toBeVisible();
  await expect(page.getByText(/App URL is private.*Unsaved settings remain in this form and were not included/)).toBeVisible();
  await expect(page.getByLabel("Public URL")).not.toBeChecked();
});

test("preserves dirty settings and the warning after a browser check", async ({ page }) => {
  const serverApp = { ...persistedSettingsApp, publicExposure: true };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1/browser-check") {
      await jsonRoute(route, { jobId: "job-browser" });
      return true;
    }
    if (path === "/api/agent-jobs/job-browser") {
      await jsonRoute(route, { id: "job-browser", status: "success" });
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");

  await actionsPanel(page).getByRole("button", { name: "Check in browser" }).click();

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByText("Unsaved settings are not included in deploys or auxiliary app actions.")).toBeVisible();
  await expect(page.getByText("Browser check completed. Unsaved settings remain in this form and were not included.")).toBeVisible();
});

test("preserves dirty settings and the warning when pausing", async ({ page }) => {
  let serverApp = { ...persistedSettingsApp, publicExposure: true, suspendedAt: null as string | null };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1/pause") {
      serverApp = { ...serverApp, suspendedAt: "2026-08-15T00:00:00Z" };
      await jsonRoute(route, {});
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");
  await actionsPanel(page).getByRole("button", { name: "Pause" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Pause" }).click();

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByText("Unsaved settings are not included in deploys or auxiliary app actions.")).toBeVisible();
  await expect(page.getByText("App paused. Unsaved settings remain in this form and were not included.")).toBeVisible();
});

test("preserves dirty settings and the warning when resuming", async ({ page }) => {
  let serverApp = { ...persistedSettingsApp, publicExposure: true, suspendedAt: "2026-08-15T00:00:00Z" as string | null };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1/resume") {
      serverApp = { ...serverApp, suspendedAt: null };
      await jsonRoute(route, {});
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");
  await actionsPanel(page).getByRole("button", { name: "Resume" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Resume" }).click();

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByText("Unsaved settings are not included in deploys or auxiliary app actions.")).toBeVisible();
  await expect(page.getByText("App resume requested. Unsaved settings remain in this form and were not included.")).toBeVisible();
});

test("preserves dirty settings and the warning when changing the build pool", async ({ page }) => {
  let serverApp = { ...persistedSettingsApp, publicExposure: true, buildPoolId: "pool-a" };
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/build-pools") {
      await jsonRoute(route, [
        { id: "pool-a", name: "Local", enabled: true, qualificationStatus: "ready" },
        { id: "pool-b", name: "Remote", enabled: true, qualificationStatus: "ready" },
      ]);
      return true;
    }
    if (path === "/api/apps/app-1/build-pool") {
      serverApp = { ...serverApp, buildPoolId: "pool-b" };
      await jsonRoute(route, {});
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");

  await page.getByLabel("Build pool").selectOption("pool-b");

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByText("Unsaved settings are not included in deploys or auxiliary app actions.")).toBeVisible();
  await expect(page.getByText("Build pool updated. Unsaved settings remain in this form and were not included.")).toBeVisible();
});

test("does not clobber a second edit while an auxiliary refresh is in flight", async ({ page }) => {
  const serverApp = { ...persistedSettingsApp, publicExposure: true };
  let appLoads = 0;
  let releaseRefresh: (() => void) | null = null;
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps/app-1" && route.request().method() === "GET") {
      appLoads += 1;
      if (appLoads > 1) await new Promise<void>((resolve) => { releaseRefresh = resolve; });
      await jsonRoute(route, serverApp);
      return true;
    }
    if (path === "/api/apps/app-1/env") {
      await jsonRoute(route, []);
      return true;
    }
    if (path === "/api/apps/app-1/browser-check") {
      await jsonRoute(route, { jobId: "job-browser" });
      return true;
    }
    if (path === "/api/agent-jobs/job-browser") {
      await jsonRoute(route, { id: "job-browser", status: "success" });
      return true;
    }
    return false;
  });
  await page.goto("/apps/app-1");
  await page.getByLabel("Domain").fill("draft.example.test");

  await actionsPanel(page).getByRole("button", { name: "Check in browser" }).click();
  await expect.poll(() => appLoads).toBeGreaterThan(1);
  await page.getByLabel("Health path").fill("/draft-health");
  (releaseRefresh as (() => void) | null)?.();

  await expect(page.getByLabel("Domain")).toHaveValue("draft.example.test");
  await expect(page.getByLabel("Health path")).toHaveValue("/draft-health");
});
