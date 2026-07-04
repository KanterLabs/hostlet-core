import { expect, test, type Page } from "@playwright/test";
import { mockApi } from "./support/mockApi";

// HCR-010 — spotlight product tour (components/ui/tour.tsx wired via
// app/tour.tsx). Browser-proves the ?tour=1 force-start, Next/Back stepping,
// Skip/Escape dismissal with the localStorage seen record, the once-per-browser
// auto-start on /, and that a seen record suppresses the auto-start.

const TOUR_KEY = "hostlet-tour";

// Seed a completed record before any page script runs, so the browser counts
// as having already seen the tour.
function seedTourSeen(page: Page) {
  return page.addInitScript(
    (key) => localStorage.setItem(key, JSON.stringify({ v: 1, completedAt: "2026-01-01T00:00:00.000Z" })),
    TOUR_KEY,
  );
}

test("?tour=1 force-starts the tour on step 1, even when already seen", async ({ page }) => {
  await mockApi(page);
  await seedTourSeen(page);
  await page.goto("/?tour=1");

  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Step 1 of 6")).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "Welcome to Hostlet" })).toBeVisible();
  // The trigger param is stripped so a reload doesn't re-run the tour.
  await expect(page).toHaveURL("/");
});

test("Next advances a step and Back returns", async ({ page }) => {
  await mockApi(page);
  await page.goto("/?tour=1");

  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("Step 1 of 6")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Back" })).toBeDisabled();

  await dialog.getByRole("button", { name: "Next" }).click();
  await expect(dialog.getByText("Step 2 of 6")).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "Health at a glance" })).toBeVisible();

  await dialog.getByRole("button", { name: "Back" }).click();
  await expect(dialog.getByText("Step 1 of 6")).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "Welcome to Hostlet" })).toBeVisible();
});

test("Skip closes the tour and records it as seen", async ({ page }) => {
  await mockApi(page);
  await page.goto("/?tour=1");

  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Skip tour" }).click();
  await expect(dialog).toHaveCount(0);

  const record = await page.evaluate((key) => localStorage.getItem(key), TOUR_KEY);
  expect(JSON.parse(record ?? "null")).toMatchObject({ v: 1 });
});

test("auto-starts on / in a fresh browser", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  // Covers the ~500ms auto-start delay plus target resolution via the
  // locator's own polling — no fixed sleeps.
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Step 1 of 6")).toBeVisible();
});

test("does not auto-start when the seen record exists", async ({ page }) => {
  await mockApi(page);
  await seedTourSeen(page);
  // Fake clock makes the negative assertion deterministic: run time forward
  // past the auto-start delay instead of sleeping wall-clock time.
  await page.clock.install();
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
  await page.clock.runFor(2000);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("Escape closes the tour and records it as seen", async ({ page }) => {
  await mockApi(page);
  await page.goto("/?tour=1");

  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  const record = await page.evaluate((key) => localStorage.getItem(key), TOUR_KEY);
  expect(JSON.parse(record ?? "null")).toMatchObject({ v: 1 });
});
