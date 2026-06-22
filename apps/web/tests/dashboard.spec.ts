import { expect, test } from "@playwright/test";
import { mockApi, textRoute } from "./support/mockApi";

// HCR-001 — self-hosted dashboard overview. Browser-proves the release/version
// metric rendering and the request-failure notice, neither of which the existing
// overview test asserts.

test("renders overview metrics and the release version", async ({ page }) => {
  await mockApi(page); // default fixtures: /api/system/version -> 0.2.0, one online machine
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
  await expect(page.getByText("Machines online")).toBeVisible();
  // Release aside renders the version from /api/system/version once it loads.
  await expect(page.getByText("Release state")).toBeVisible();
  await expect(page.getByText("0.2.0")).toBeVisible();
});

test("shows a warning notice when a data request fails", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/apps") {
      await textRoute(route, "control plane unreachable", 500);
      return true;
    }
    return false;
  });
  await page.goto("/");

  await expect(page.getByText("control plane unreachable")).toBeVisible();
});
