import { expect, test, type Page, type Route } from "@playwright/test";

test("self-hosted overview and create flow stay available", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
  await expect(page.getByText("Docker + Caddy")).toBeVisible();

  await page.goto("/apps/new");
  await expect(page.getByRole("heading", { name: "Create app" })).toBeVisible();
  await expect(page.getByText("Local target and route")).toBeVisible();
});

test("settings show self-hosted provider status", async ({ page }) => {
  await mockApi(page);
  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByText("GitHub Device Flow is configured.")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Cloudflare" })).toBeVisible();
});

// Deterministic id for the synthetic "This machine" server so the fixture
// stays stable across runs.
const LOCAL_SERVER_ID = "00000000-0000-0000-0000-000000000001";

// Maps a backend path to the JSON body the mock should return. Several paths
// share a fixture, so they are grouped onto the same entry.
const API_FIXTURES: Record<string, unknown> = {
  "/api/session": { authenticated: true, mode: "self_hosted", cloud: null, unlocked: true },
  "/api/setup/status": { authenticated: true, mode: "self_hosted", cloud: null, unlocked: true },
  "/api/apps": [],
  "/api/servers": [{ id: LOCAL_SERVER_ID, name: "This machine", kind: "local", status: "online" }],
  "/api/github/status": { oauthConfigured: true, webhookConfigured: true, authenticated: true, tokenValid: true, login: "ci-user", message: "GitHub Device Flow is configured." },
  "/api/github/repos": [],
  "/api/cloudflare/status": { configured: true, tokenValid: true, baseDomain: "example.test", defaultDomainPattern: "*.example.test", domainPrefix: "hostlet-", tunnelTargetConfigured: true, message: "Cloudflare DNS is configured." },
  "/api/system/version": { currentVersion: "0.2.0", updateChecksEnabled: true },
  "/api/system/cleanup": { database: {}, docker: { keepContainers: 1, keepImages: 1, jobWillRun: true } },
  "/api/system/backups/latest": null,
  "/api/agent-jobs": [],
  "/api/audit-events": [],
};

function isBackendPath(path: string): boolean {
  return path.startsWith("/api") || path.startsWith("/auth");
}

async function mockApi(page: Page) {
  await page.route("**/*", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (!isBackendPath(path)) return route.continue();
    return handleApi(route, path);
  });
}

async function handleApi(route: Route, path: string) {
  if (path in API_FIXTURES) {
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(API_FIXTURES[path]),
    });
  }
  return route.fulfill({ status: 404, body: "not mocked" });
}
