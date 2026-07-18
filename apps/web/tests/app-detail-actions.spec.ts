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
