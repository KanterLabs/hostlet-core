import { expect, test, type Route } from "@playwright/test";
import { jsonRoute, mockApi, textRoute } from "./support/mockApi";

// HCR-002 — setup/unlock gate (AuthGate). The default fixture returns
// unlocked:true (gate passes through), so each test overrides /api/setup/status
// to exercise a branch that was previously not browser-proven: the unlock form,
// the first-run setup form, client-side password-mismatch validation, a failed
// unlock, and an unreachable setup-status endpoint.

const setupStatus = (status: { setupRequired: boolean; unlocked: boolean }) =>
  async (route: Route, path: string): Promise<boolean> => {
    if (path === "/api/setup/status") {
      await jsonRoute(route, { mode: "self_hosted", ...status });
      return true;
    }
    return false;
  };

test("locked control plane shows the unlock form", async ({ page }) => {
  await mockApi(page, setupStatus({ setupRequired: false, unlocked: false }));
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Unlock Hostlet" })).toBeVisible();
  await expect(page.getByLabel("Password", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Unlock" })).toBeVisible();
  // Setup-only fields must not appear when unlocking an already-configured machine.
  await expect(page.getByLabel("Confirm password")).toHaveCount(0);
  await expect(page.getByLabel("Setup token")).toHaveCount(0);
});

test("first-run setup shows the secure form with confirm + token", async ({ page }) => {
  await mockApi(page, setupStatus({ setupRequired: true, unlocked: false }));
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Secure Hostlet" })).toBeVisible();
  await expect(page.getByLabel("Confirm password")).toBeVisible();
  await expect(page.getByLabel("Setup token")).toBeVisible();
  await expect(page.getByRole("button", { name: "Set password" })).toBeVisible();
});

test("setup rejects mismatched passwords client-side", async ({ page }) => {
  // No /api/setup mock: a mismatch must short-circuit before any request.
  await mockApi(page, setupStatus({ setupRequired: true, unlocked: false }));
  await page.goto("/");

  await page.getByLabel("Password", { exact: true }).fill("correct-horse-battery");
  await page.getByLabel("Confirm password").fill("different-horse-battery");
  await page.getByRole("button", { name: "Set password" }).click();

  await expect(page.getByText("Passwords do not match.")).toBeVisible();
});

test("failed unlock surfaces the API error message", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/setup/status") {
      await jsonRoute(route, { mode: "self_hosted", setupRequired: false, unlocked: false });
      return true;
    }
    if (path === "/api/unlock" && route.request().method() === "POST") {
      await textRoute(route, "Incorrect control-plane password.", 401);
      return true;
    }
    return false;
  });
  await page.goto("/");

  await page.getByLabel("Password", { exact: true }).fill("wrong-password-1234");
  await page.getByRole("button", { name: "Unlock" }).click();

  await expect(page.getByText("Incorrect control-plane password.")).toBeVisible();
});

test("unreachable setup-status surfaces a connection error", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/setup/status") {
      await textRoute(route, "control plane offline", 500);
      return true;
    }
    return false;
  });
  await page.goto("/");

  await expect(page.getByText("Checking control-plane security status...")).toBeVisible();
  await expect(page.getByText("control plane offline")).toBeVisible();
});
