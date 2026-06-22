import type { Page, Route } from "@playwright/test";

// Shared Playwright backend-mock harness for the self-hosted web specs. The
// browser never talks to a real API: every /api or /auth request is fulfilled
// from API_FIXTURES, with an optional per-test override that runs first.

// Deterministic id for the synthetic "This machine" server so fixtures stay
// stable across runs.
export const LOCAL_SERVER_ID = "00000000-0000-0000-0000-000000000001";

// Maps a backend path to the JSON body the mock returns. Several paths share a
// fixture, so they are grouped onto the same entry.
export const API_FIXTURES: Record<string, unknown> = {
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

export function isBackendPath(path: string): boolean {
  return path.startsWith("/api") || path.startsWith("/auth");
}

export async function mockApi(page: Page, override?: (route: Route, path: string) => Promise<boolean>) {
  await page.route("**/*", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (!isBackendPath(path)) return route.continue();
    if (await override?.(route, path)) return;
    return handleApi(route, path);
  });
}

export async function handleApi(route: Route, path: string) {
  if (path in API_FIXTURES) {
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(API_FIXTURES[path]),
    });
  }
  return route.fulfill({ status: 404, body: "not mocked" });
}

// Fulfill a single intercepted route with a JSON body (status 200 by default).
// Lets per-test overrides stay one-liners: `await jsonRoute(route, {...}); return true;`
export function jsonRoute(route: Route, body: unknown, status = 200) {
  return route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}

// Fulfill a route with a plain-text error body (for non-OK API responses).
export function textRoute(route: Route, body: string, status: number) {
  return route.fulfill({ status, contentType: "text/plain", body });
}
