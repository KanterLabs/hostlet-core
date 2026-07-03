import { defineConfig, devices } from "@playwright/test";
import { existsSync } from "node:fs";

const port = Number(process.env.HOSTLET_WEB_TEST_PORT || 13002);
const outputDir = process.env.HOSTLET_PLAYWRIGHT_OUTPUT_DIR || "/tmp/hostlet-core-playwright-results";
const systemChromium = "/usr/bin/chromium-browser";
const chromiumExecutablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
  || (existsSync(systemChromium) ? systemChromium : undefined);

export default defineConfig({
  testDir: "./tests",
  outputDir,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    ...devices["Desktop Chrome"],
    launchOptions: chromiumExecutablePath ? { executablePath: chromiumExecutablePath } : undefined,
  },
  webServer: {
    command: `mkdir -p .next/standalone/.next && cp -R .next/static .next/standalone/.next/static && HOSTNAME=127.0.0.1 PORT=${port} node .next/standalone/server.js`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    env: {
      NEXT_PUBLIC_API_URL: process.env.NEXT_PUBLIC_API_URL || "http://127.0.0.1:18080",
      NEXT_PUBLIC_WEBHOOK_URL: process.env.NEXT_PUBLIC_WEBHOOK_URL || "http://127.0.0.1:18080",
    },
  },
});
