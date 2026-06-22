import { expect, test, type Route } from "@playwright/test";
import { jsonRoute, mockApi, textRoute } from "./support/mockApi";

// HCR-003 — GitHub device flow + repo listing. Browser-proves the login-page
// device flow (start -> pending code/link display -> authorized redirect, and an
// expired poll exposing the restart affordance) and the create-app repo-listing
// load failure on /api/github/repos, none of which were previously asserted.
//
// The poll effect never fires faster than GitHub's 5s minimum, so assertions that
// depend on a poll result use an explicit timeout above the 5s expect default.
const POLL_TIMEOUT = 15_000;

type PollBody = {
  status: "pending" | "authorized" | "expired" | "denied";
  message: string;
  redirectTo?: string;
};

// DeviceStart payload returned by /auth/github/device/start. interval 5 keeps the
// poll cadence at the component's MIN_POLL_INTERVAL_S floor.
const deviceStart = {
  flowId: "flow-123",
  userCode: "WDJB-MJHT",
  verificationUri: "https://github.com/login/device",
  verificationUriComplete: "https://github.com/login/device?user_code=WDJB-MJHT",
  expiresIn: 900,
  interval: 5,
};

// Override that drives the device flow: start always succeeds; every poll returns
// the supplied result.
const driveDeviceFlow = (poll: PollBody) =>
  async (route: Route, path: string): Promise<boolean> => {
    if (path === "/auth/github/device/start") {
      await jsonRoute(route, deviceStart);
      return true;
    }
    if (path === "/auth/github/device/poll") {
      await jsonRoute(route, poll);
      return true;
    }
    return false;
  };

test("starting the device flow shows the user code and verification link", async ({ page }) => {
  await mockApi(page, driveDeviceFlow({ status: "pending", message: "Waiting for GitHub authorization." }));
  await page.goto("/login");

  await page.getByRole("button", { name: "Continue with GitHub" }).click();

  await expect(page.getByText("GitHub code")).toBeVisible();
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();
  await expect(page.getByText("Waiting for GitHub authorization.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Open GitHub" })).toHaveAttribute(
    "href",
    "https://github.com/login/device?user_code=WDJB-MJHT",
  );
});

test("an authorized poll redirects to the captured target", async ({ page }) => {
  await mockApi(page, driveDeviceFlow({ status: "authorized", message: "GitHub authorized. Redirecting...", redirectTo: "/apps" }));
  await page.goto("/login");

  await page.getByRole("button", { name: "Continue with GitHub" }).click();
  // Pending is reached synchronously after start, before the first poll fires.
  await expect(page.getByText("Waiting for GitHub authorization.")).toBeVisible();

  await expect(page).toHaveURL(/\/apps/, { timeout: POLL_TIMEOUT });
});

test("an expired poll surfaces the message and a restart affordance", async ({ page }) => {
  await mockApi(page, driveDeviceFlow({ status: "expired", message: "This device code expired. Start over." }));
  await page.goto("/login");

  await page.getByRole("button", { name: "Continue with GitHub" }).click();

  await expect(page.getByText("This device code expired. Start over.")).toBeVisible({ timeout: POLL_TIMEOUT });
  await expect(page.getByRole("button", { name: "Start again" })).toBeVisible({ timeout: POLL_TIMEOUT });
});

test("a repo-listing failure surfaces a load error on the create-app page", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/github/repos") {
      await textRoute(route, "GitHub token expired.", 500);
      return true;
    }
    return false;
  });
  await page.goto("/apps/new");

  await expect(page.getByText(/Could not load repos\. GitHub token expired\./)).toBeVisible();
});
