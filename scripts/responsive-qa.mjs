#!/usr/bin/env node
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";

const root = new URL("..", import.meta.url).pathname;
const requireFromWeb = createRequire(new URL("../apps/web/package.json", import.meta.url));
const { chromium } = requireFromWeb("@playwright/test");
const port = process.env.HOSTLET_RESPONSIVE_QA_PORT || "13001";
const baseUrl = `http://127.0.0.1:${port}`;
const widths = [320, 375, 768, 1024, 1440];
const routes = [
  ["/", "nav"],
  ["/apps/new", "create app"],
  ["/apps/smoke-app", "app detail and env editor"],
  ["/settings", "settings"],
  ["/deployments/smoke-deployment", "deployment logs"],
];

await mkdir("/tmp/hostlet-responsive-qa", { recursive: true });

const server = spawn(
  "pnpm",
  ["--dir", "apps/web", "exec", "next", "start", "-H", "127.0.0.1", "-p", port],
  {
    cwd: root,
    env: {
      ...process.env,
      NEXT_PUBLIC_API_URL: process.env.NEXT_PUBLIC_API_URL || "http://127.0.0.1:18080",
      NEXT_PUBLIC_WEBHOOK_URL: process.env.NEXT_PUBLIC_WEBHOOK_URL || "http://127.0.0.1:18080",
    },
    stdio: ["ignore", "pipe", "pipe"],
  },
);

let serverOutput = "";
server.stdout.on("data", (chunk) => {
  serverOutput += chunk.toString();
});
server.stderr.on("data", (chunk) => {
  serverOutput += chunk.toString();
});

try {
  await waitForServer(baseUrl);
  const browser = await chromium.launch();
  try {
    for (const width of widths) {
        const page = await browser.newPage({ viewport: { width, height: 900 } });
        const errors = [];
        page.on("console", (message) => {
          if (message.type() !== "error") return;
          const text = message.text();
          if (text.includes("/ws/logs/") && text.includes("ERR_CONNECTION_REFUSED")) return;
          errors.push(text);
        });
        page.on("pageerror", (error) => errors.push(error.message));
        await installApiMocks(page);
        for (const [route, label] of routes) {
          await page.goto(`${baseUrl}${route}`, { waitUntil: "networkidle" });
          await page.screenshot({
            path: `/tmp/hostlet-responsive-qa/${width}-${safeName(label)}.png`,
            fullPage: true,
          });
          const result = await page.evaluate(() => {
            const overflow = document.documentElement.scrollWidth - window.innerWidth;
            const mainText = document.querySelector("main")?.textContent?.trim() || "";
            const badTextContainers = Array.from(
              document.querySelectorAll("button, a.button, .button-secondary, .button-danger, input, select"),
            )
              .filter((element) => {
                const el = element;
                const rect = el.getBoundingClientRect();
                if (rect.width <= 0 || rect.height <= 0) return false;
                return el.scrollWidth > el.clientWidth + 2;
              })
              .map((element) => element.textContent?.trim() || element.getAttribute("placeholder") || element.tagName);
            return {
              overflow,
              mainTextLength: mainText.length,
              badTextContainers: badTextContainers.slice(0, 5),
            };
          });
          if (errors.length) {
            throw new Error(`${label} at ${width}px logged browser errors: ${errors.slice(0, 3).join(" | ")}`);
          }
          if (result.mainTextLength < 20) {
            throw new Error(`${label} at ${width}px rendered blank primary content`);
          }
          if (result.overflow > 2) {
            throw new Error(`${label} at ${width}px has horizontal overflow of ${result.overflow}px`);
          }
          if (result.badTextContainers.length) {
            throw new Error(
              `${label} at ${width}px has clipped control text: ${result.badTextContainers.join(", ")}`,
            );
          }
        }
        await page.close();
    }
  } finally {
    await browser.close();
  }
} catch (error) {
  console.error(serverOutput);
  console.error(error);
  process.exitCode = 1;
} finally {
  server.kill("SIGTERM");
}

async function waitForServer(url) {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // Keep waiting.
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(`Timed out waiting for ${url}`);
}

function safeName(value) {
  return value.replace(/[^a-z0-9]+/gi, "-").replace(/^-|-$/g, "").toLowerCase();
}

async function installApiMocks(page) {
  const app = {
    id: "smoke-app",
    name: "Smoke App",
    repoFullName: "hostlet-ci/node-hello",
    branch: "main",
    domain: "localhost:23000",
    currentDeploymentId: "smoke-deployment",
    runtimeKind: "single",
    hostletConfigPath: "hostlet.yml",
    runtimeConfig: {},
    packagingStrategy: "generated",
    rootDirectory: ".",
    installCommand: null,
    buildCommand: null,
    startCommand: null,
    containerPort: 3000,
    healthPath: "/health",
    memoryLimitMb: 1024,
    cpuLimit: 1,
    publicExposure: true,
    autoDeploy: true,
    server: {
      id: "00000000-0000-0000-0000-000000000001",
      name: "This machine",
      kind: "local",
      status: "online",
    },
    latestDeployment: {
      id: "smoke-deployment",
      status: "success",
      commitSha: "1234567890abcdef",
      failure: null,
      startedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
      runtimeMetadata: {
        packagingStrategy: "generated",
        generatedDockerfile: false,
        buildBackend: "railpack",
        detectedFramework: null,
        runtimeKind: null,
        packageManager: null,
        buildDurationMs: 293000,
        imageSizeBytes: 149422080,
      },
    },
    currentDeployment: {
      status: "success",
      publishedPort: 32001,
      finishedAt: new Date().toISOString(),
    },
    latestWebhook: null,
    health: {
      status: "healthy",
      httpStatus: 200,
      latencyMs: 24,
      failureCount: 0,
      successCount: 4,
      lastError: null,
      lastCheckedAt: new Date().toISOString(),
      lastHealthyAt: new Date().toISOString(),
    },
  };
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });

    if (path === "/api/setup/status") return json({ mode: "self_hosted", setupRequired: false, unlocked: true });
    if (path === "/api/session") {
      return json({
        mode: "self_hosted",
        user: { id: "ci-user", login: "ci-user" },
      });
    }
    if (path === "/api/github/status") return json({ oauthConfigured: true, webhookConfigured: true, authenticated: true, tokenValid: true, login: "ci-user", message: "GitHub connected." });
    if (path === "/api/github/repos") return json([{ full_name: "hostlet-ci/node-hello", private: false, default_branch: "main" }]);
    if (path === "/api/servers") {
      return json([
        {
          id: "00000000-0000-0000-0000-000000000001",
          name: "This machine",
          kind: "local",
          status: "online",
          version: "0.2.0",
          lastSeenAt: new Date().toISOString(),
        },
      ]);
    }
    if (path === "/api/cloudflare/status") {
      return json({
        configured: true,
        baseDomain: "example.test",
        defaultDomainPattern: "hostlet-*.example.test",
      });
    }
    if (path === "/api/apps") return json([app]);
    if (path === "/api/apps/smoke-app") return json(app);
    if (path === "/api/apps/smoke-app/env") return json([{ key: "APP_VERSION" }, { key: "DATABASE_URL" }]);
    if (path === "/api/apps/smoke-app/resources") {
      return json({
        cpuPercent: "1.23%",
        memoryUsage: "64MiB / 512MiB",
        memoryPercent: "12.5%",
        networkIo: "1kB / 2kB",
        blockIo: "0B / 0B",
        pids: "12",
        sampledAt: new Date().toISOString(),
      });
    }
    if (path === "/api/apps/smoke-app/health") return json(app.health);
    if (path === "/api/apps/smoke-app/health/events") return json([]);
    if (path === "/api/deployments/smoke-deployment") {
      return json({ id: "smoke-deployment", appId: "smoke-app", status: "success", commitSha: "1234567890abcdef", failure: null, runtimeMetadata: app.latestDeployment.runtimeMetadata });
    }
    if (path === "/api/deployments/smoke-deployment/logs") {
      return json([
        { stream: "stdout", line: "Building image" },
        { stream: "stdout", line: "Health check passed." },
      ]);
    }
    if (path === "/api/health/summary") return json({ healthy: 1, degraded: 0, unhealthy: 0, unknown: 0 });
    if (path === "/api/audit-events") return json([]);
    if (path === "/api/agent-jobs") return json([]);
    if (path === "/api/system/version") {
      return json({
        currentVersion: "0.2.0",
        updateChecksEnabled: true,
        update: {
          latestVersion: "0.2.0",
          updateAvailable: false,
          checkedAt: new Date().toISOString(),
        },
      });
    }
    if (path === "/api/system/backups/latest") return json({ latest: null });
    if (path === "/api/system/operator-status") return json({ ok: true });
    if (path === "/api/system/cleanup") {
      return json({
        database: {
          deployments: 0,
          health_events: 0,
          agent_jobs: 0,
          audit_events: 0,
        },
        docker: {
          keepContainers: 1,
          keepImages: 1,
          jobWillRun: true,
        },
      });
    }
    return json({});
  });
}
