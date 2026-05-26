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
      for (const [route, label] of routes) {
        await page.goto(`${baseUrl}${route}`, { waitUntil: "networkidle" });
        await page.screenshot({
          path: `/tmp/hostlet-responsive-qa/${width}-${safeName(label)}.png`,
          fullPage: true,
        });
        const result = await page.evaluate(() => {
          const overflow = document.documentElement.scrollWidth - window.innerWidth;
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
            badTextContainers: badTextContainers.slice(0, 5),
          };
        });
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
