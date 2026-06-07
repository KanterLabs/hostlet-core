import { defineConfig, devices } from "@playwright/test";

const port = Number(process.env.HOSTLET_WEB_TEST_PORT || 13002);
const outputDir = process.env.HOSTLET_PLAYWRIGHT_OUTPUT_DIR || "/tmp/hostlet-core-playwright-results";

export default defineConfig({
  testDir: "./tests",
  outputDir,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    ...devices["Desktop Chrome"],
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
