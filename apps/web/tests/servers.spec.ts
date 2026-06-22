import { expect, test } from "@playwright/test";
import { jsonRoute, mockApi, textRoute } from "./support/mockApi";

// HCR-008 — machines page (server list). Browser-proves the machine list, the
// derived online/local counts, and the empty + error states that were not
// previously asserted. (The agent install command is a backend script endpoint,
// not part of this page's UI.)

test("lists machines with status, kind, and derived online count", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/servers") {
      await jsonRoute(route, [
        { id: "s1", name: "build-vm", kind: "local", status: "online", lastSeenAt: "2026-06-21T10:00:00Z" },
        { id: "s2", name: "edge-1", kind: "remote", status: "offline", publicIp: "203.0.113.5", lastSeenAt: null },
      ]);
      return true;
    }
    return false;
  });
  await page.goto("/servers");

  await expect(page.getByRole("heading", { name: "build-vm" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "edge-1" })).toBeVisible();
  await expect(page.getByText("offline", { exact: true })).toBeVisible();
  await expect(page.getByText("remote", { exact: true })).toBeVisible();
  // "Online agents" metric = online / total.
  await expect(page.getByText("1/2")).toBeVisible();
});

test("shows an empty state when no machines are present", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/servers") {
      await jsonRoute(route, []);
      return true;
    }
    return false;
  });
  await page.goto("/servers");

  await expect(page.getByText("No machines yet.")).toBeVisible();
});

test("shows an error state when the machines request fails", async ({ page }) => {
  await mockApi(page, async (route, path) => {
    if (path === "/api/servers") {
      await textRoute(route, "machines unreachable", 500);
      return true;
    }
    return false;
  });
  await page.goto("/servers");

  await expect(page.getByText(/Could not load machines\./)).toBeVisible();
});
