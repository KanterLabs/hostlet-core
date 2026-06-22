import { expect, test } from "@playwright/test";
import { jsonRoute, mockApi, textRoute } from "./support/mockApi";

// HCR-005 — apps list. Browser-proves status filtering, the empty state, a
// transport failure (distinct from empty), and the auth-failure path (a 401
// redirects to /login), none of which were previously asserted.

const server = { id: "s", name: "local", kind: "local", status: "online" };
const APPS = [
  { id: "a1", name: "billing-api", repoFullName: "acme/billing", branch: "main", rootDirectory: ".", latestDeployment: { id: "d1", status: "building" }, health: { status: "unknown" }, server },
  { id: "a2", name: "marketing-web", repoFullName: "acme/marketing", branch: "main", rootDirectory: ".", latestDeployment: { id: "d2", status: "failed", failure: "build failed" }, health: { status: "unhealthy" }, server },
  { id: "a3", name: "analytics-db", repoFullName: "acme/analytics", branch: "main", rootDirectory: ".", latestDeployment: { id: "d3", status: "success" }, health: { status: "healthy" }, server },
];

test("filters the app list by status", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps") {
      await jsonRoute(route, APPS);
      return true;
    }
    return false;
  });
  await page.goto("/apps");

  await expect(page.getByRole("link", { name: "billing-api" })).toBeVisible();
  await expect(page.getByRole("link", { name: "marketing-web" })).toBeVisible();

  await page.getByRole("button", { name: "failed", exact: true }).click();
  await expect(page.getByRole("link", { name: "marketing-web" })).toBeVisible();
  await expect(page.getByRole("link", { name: "billing-api" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "analytics-db" })).toHaveCount(0);

  await page.getByRole("button", { name: "healthy", exact: true }).click();
  await expect(page.getByRole("link", { name: "analytics-db" })).toBeVisible();
  await expect(page.getByRole("link", { name: "marketing-web" })).toHaveCount(0);
});

test("shows the empty state when there are no apps", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps") {
      await jsonRoute(route, []);
      return true;
    }
    return false;
  });
  await page.goto("/apps");

  await expect(page.getByText("No apps yet.")).toBeVisible();
});

test("shows a transport error distinct from the empty state", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps") {
      await textRoute(route, "api unreachable", 500);
      return true;
    }
    return false;
  });
  await page.goto("/apps");

  await expect(page.getByText(/Could not load apps\. api unreachable/)).toBeVisible();
});

test("auth failure redirects to the login page", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps") {
      await textRoute(route, "unauthorized", 401);
      return true;
    }
    return false;
  });
  await page.goto("/apps");

  await expect(page).toHaveURL(/\/login/);
});
