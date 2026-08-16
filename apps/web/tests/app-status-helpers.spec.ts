import { expect, test } from "@playwright/test";

import { healthMetricDetail, isActiveDeploy, isTerminalDeploy, shortSha } from "@/lib/app-status";
import { statusChartColor, statusToneClass } from "@/components/ui/status";

// Pure-logic unit coverage for the consolidated status helpers. These do not
// touch the network or the DOM, so they run without the dev server.
test.describe("app-status helpers", () => {
  for (const status of ["queued", "running", "building", "starting", "health_checking", "routing"]) {
    test(`isActiveDeploy is true for ${status}`, () => {
      expect(isActiveDeploy(status)).toBe(true);
    });
  }

  test("isActiveDeploy is false for an inactive/terminal status", () => {
    expect(isActiveDeploy("succeeded")).toBe(false);
  });

  test("isActiveDeploy is false for null/empty", () => {
    expect(isActiveDeploy(null)).toBe(false);
    expect(isActiveDeploy(undefined)).toBe(false);
    expect(isActiveDeploy("")).toBe(false);
  });

  for (const status of ["success", "failed", "canceled", "cancelled", "rolled_back"]) {
    test(`isTerminalDeploy is true for ${status}`, () => {
      expect(isTerminalDeploy(status)).toBe(true);
    });
  }

  test("isTerminalDeploy is false while a deployment is still running", () => {
    expect(isTerminalDeploy("building")).toBe(false);
  });

  test("rolled-back deployments use the terminal danger pill treatment", () => {
    expect(statusToneClass("rolled_back")).toContain("bg-danger-bg");
    expect(statusChartColor("rolled_back")).toBe("hsl(var(--danger-fg))");
  });

  test("shortSha truncates to 7 chars and handles sentinels", () => {
    expect(shortSha("0123456789abcdef")).toBe("0123456");
    expect(shortSha("HEAD")).toBe("HEAD");
    expect(shortSha(null)).toBe("No deploy yet");
    expect(shortSha(undefined)).toBe("No deploy yet");
  });

  test("browser startup failures take precedence over HTTP health detail", () => {
    expect(healthMetricDetail({
      browser: { failure: "browser smoke rejected: uncaught page error" },
      lastError: "HTTP 503",
      latencyMs: 42,
    })).toBe("browser smoke rejected: uncaught page error");
  });
});
