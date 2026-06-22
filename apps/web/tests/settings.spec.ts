import { expect, test } from "@playwright/test";
import { jsonRoute, mockApi, textRoute } from "./support/mockApi";

// HCR-009 — settings hub. Browser-proves the per-action busy/feedback states for
// the two control-plane actions ("Check for updates", "Run cleanup"): each
// disables its button while the request is in flight, renders its transient
// progress notice, and then renders the success notice the hook sets in its
// finally/then. The in-flight window is held open with a short route delay so the
// busy state is observable instead of racy. A failed cleanup proves the danger
// notice surfaces the API error text. The default fixtures already serve every
// refresh() fetch, so each test only overrides the one POST endpoint it exercises.

const HOLD_MS = 600;
const hold = () => new Promise((resolve) => setTimeout(resolve, HOLD_MS));

test("checks for updates and reports the control plane is current", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/system/update-check" && route.request().method() === "POST") {
      await hold();
      await jsonRoute(route, { updateAvailable: false, latestVersion: "0.2.0", checkedAt: "2026-06-20T00:00:00Z" });
      return true;
    }
    return false;
  });
  await page.goto("/settings");

  const check = page.getByRole("button", { name: "Check for updates" });
  await check.click();

  // While the POST is held open the hook flips busy=true and posts a progress notice.
  await expect(page.getByText("Checking for updates...")).toBeVisible();
  await expect(check).toBeDisabled();

  // After the response resolves busy clears and the up-to-date notice replaces it.
  await expect(page.getByText("Hostlet is up to date.")).toBeVisible();
  await expect(check).toBeEnabled();
});

test("surfaces an available update from the check action", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/system/update-check" && route.request().method() === "POST") {
      await jsonRoute(route, { updateAvailable: true, latestVersion: "0.3.0", checkedAt: "2026-06-20T00:00:00Z" });
      return true;
    }
    return false;
  });
  await page.goto("/settings");

  await page.getByRole("button", { name: "Check for updates" }).click();

  await expect(page.getByText("Update available. Run hostlet update on the server.")).toBeVisible();
  // The returned update payload is folded into version state and rendered.
  await expect(page.getByText("0.3.0")).toBeVisible();
});

test("runs cleanup and confirms the docker job will follow", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    // Only the POST is overridden; the GET cleanup plan (initial load + the
    // post-success refresh) falls through to the default fixture.
    if (path === "/api/system/cleanup" && route.request().method() === "POST") {
      await hold();
      await jsonRoute(route, {});
      return true;
    }
    return false;
  });
  await page.goto("/settings");

  const run = page.getByRole("button", { name: "Run cleanup" });
  await run.click();

  await expect(page.getByText("Cleanup requested...")).toBeVisible();
  await expect(run).toBeDisabled();

  await expect(page.getByText("Cleanup started. Docker cleanup will appear as an agent job.")).toBeVisible();
  await expect(run).toBeEnabled();
});

test("shows a danger notice when cleanup fails", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/system/cleanup" && route.request().method() === "POST") {
      await textRoute(route, "docker daemon unreachable", 500);
      return true;
    }
    return false;
  });
  await page.goto("/settings");

  await page.getByRole("button", { name: "Run cleanup" }).click();

  // lib/api throws Error(responseText) on a non-OK response; the hook surfaces it.
  await expect(page.getByText("docker daemon unreachable")).toBeVisible();
});
