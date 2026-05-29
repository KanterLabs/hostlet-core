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

async function mockApi(page: Page) {
  await page.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (!url.pathname.startsWith("/api") && !url.pathname.startsWith("/auth")) return route.continue();
    return handleApi(route, url.pathname);
  });
}

async function handleApi(route: Route, path: string) {
  const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  if (path === "/api/session" || path === "/api/setup/status") return json({ authenticated: true, mode: "self_hosted", cloud: null, unlocked: true });
  if (path === "/api/apps") return json([]);
  if (path === "/api/servers") return json([{ id: "00000000-0000-0000-0000-000000000001", name: "This machine", kind: "local", status: "online" }]);
  if (path === "/api/github/status") return json({ oauthConfigured: true, webhookConfigured: true, authenticated: true, tokenValid: true, login: "ci-user", message: "GitHub Device Flow is configured." });
  if (path === "/api/github/repos") return json([]);
  if (path === "/api/cloudflare/status") return json({ configured: true, tokenValid: true, baseDomain: "example.test", defaultDomainPattern: "*.example.test", domainPrefix: "hostlet-", tunnelTargetConfigured: true, message: "Cloudflare DNS is configured." });
  if (path === "/api/system/version") return json({ currentVersion: "0.5.1", updateChecksEnabled: true });
  if (path === "/api/system/cleanup") return json({ database: {}, docker: { keepContainers: 1, keepImages: 1, jobWillRun: true } });
  if (path === "/api/system/backups/latest") return json(null);
  if (path === "/api/agent-jobs" || path === "/api/audit-events") return json([]);
  return route.fulfill({ status: 404, body: "not mocked" });
}
