const fs = require("fs");
const dns = require("dns").promises;
const net = require("net");
const { chromium } = require("playwright-core");

const [targetUrl, outputPath] = process.argv.slice(2);
if (!targetUrl || !outputPath) {
  console.error("usage: capture.js <url> <output-path>");
  process.exit(2);
}

const match = /^(\d+)x(\d+)$/.exec(process.env.HOSTLET_SCREENSHOT_SIZE || "1280x720");
const width = match ? Number(match[1]) : 1280;
const height = match ? Number(match[2]) : 720;

// The capture target's own origin is always allowed because self-hosted apps
// legitimately resolve to local addresses (split-horizon DNS, host-published
// ports). Every OTHER request (redirect hops, subresources, fetches) must be
// publicly routable, so any host resolving to a private/loopback/link-local/
// CGNAT address is blocked. Unknown or unparseable input fails closed.
function isBlockedIp(ip) {
  const kind = net.isIP(ip);
  if (kind === 4) {
    const octets = ip.split(".").map(Number);
    const [a, b] = octets;
    if (a === 0 || a === 10 || a === 127 || a >= 224) return true;
    if (a === 169 && b === 254) return true;
    if (a === 172 && b >= 16 && b <= 31) return true;
    if (a === 192 && b === 168) return true;
    if (a === 100 && b >= 64 && b <= 127) return true;
    if (octets.every((part) => part === 255)) return true;
    return false;
  }
  if (kind === 6) {
    const lower = ip.toLowerCase();
    const mapped = ipv4FromMappedIpv6(lower);
    if (mapped) return isBlockedIp(mapped);
    if (lower === "::" || lower === "::1") return true;
    const firstGroup = lower.split(":")[0];
    if (firstGroup.startsWith("fc") || firstGroup.startsWith("fd")) return true;
    const firstHextet = parseInt(firstGroup, 16) || 0;
    if ((firstHextet & 0xffc0) === 0xfe80) return true;
    if (lower.startsWith("ff")) return true;
    return false;
  }
  return true;
}

function ipv4FromMappedIpv6(ip) {
  const dotted = /^(.*:)ffff:(\d+\.\d+\.\d+\.\d+)$/.exec(ip);
  if (dotted) return dotted[2];

  const groups = expandIpv6(ip);
  if (!groups) return null;
  if (
    groups.slice(0, 5).every((group) => group === 0) &&
    groups[5] === 0xffff
  ) {
    const hi = groups[6];
    const lo = groups[7];
    return `${hi >> 8}.${hi & 0xff}.${lo >> 8}.${lo & 0xff}`;
  }
  return null;
}

function expandIpv6(ip) {
  if (ip.includes(".")) return null;
  const parts = ip.split("::");
  if (parts.length > 2) return null;
  const left = parts[0] ? parts[0].split(":") : [];
  const right = parts.length === 2 && parts[1] ? parts[1].split(":") : [];
  if (parts.length === 1 && left.length !== 8) return null;
  const missing = 8 - left.length - right.length;
  if (missing < 0 || (parts.length === 1 && missing !== 0)) return null;
  const rawGroups = [...left, ...Array(missing).fill("0"), ...right];
  if (rawGroups.length !== 8) return null;
  const groups = rawGroups.map((group) => {
    if (!/^[0-9a-f]{1,4}$/.test(group)) return Number.NaN;
    return parseInt(group, 16);
  });
  return groups.some(Number.isNaN) ? null : groups;
}

async function isBlockedUrl(url, lookupCache) {
  const host = url.hostname.replace(/^\[|\]$/g, "");
  if (net.isIP(host)) {
    return isBlockedIp(host);
  }
  let addresses = lookupCache.get(host);
  if (!addresses) {
    try {
      addresses = await dns.lookup(host, { all: true, verbatim: true });
    } catch {
      addresses = [];
    }
    lookupCache.set(host, addresses);
  }
  return addresses.length === 0 || addresses.some((entry) => isBlockedIp(entry.address));
}

async function rejectBlockedRedirects(startUrl, allowedOrigin, lookupCache) {
  let current = new URL(startUrl);
  for (let hop = 0; hop < 10; hop += 1) {
    const response = await fetch(current, { redirect: "manual" });
    if (response.status < 300 || response.status >= 400) {
      return;
    }
    const location = response.headers.get("location");
    if (!location) {
      throw new Error(`redirect from ${current.origin} did not include a Location header`);
    }
    const next = new URL(location, current);
    if (next.protocol !== "http:" && next.protocol !== "https:") {
      throw new Error(`blocked redirect to unsupported protocol ${next.protocol}`);
    }
    if (next.origin !== allowedOrigin && (await isBlockedUrl(next, lookupCache))) {
      throw new Error(`blocked request to ${next.origin} (resolves to a private or local address)`);
    }
    current = next;
  }
  throw new Error("too many redirects while validating screenshot target");
}

async function main() {
  const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;
  const browser = await chromium.launch({
    headless: true,
    ...(executablePath ? { executablePath } : {}),
  });
  try {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      serviceWorkers: "block",
    });
    const page = await context.newPage();

    let allowedOrigin = null;
    try {
      const target = new URL(targetUrl);
      if (target.protocol === "http:" || target.protocol === "https:") {
        allowedOrigin = target.origin;
      }
    } catch {
      allowedOrigin = null;
    }

    const lookupCache = new Map();
    await context.route("**/*", async (route) => {
      try {
        const url = new URL(route.request().url());
        if (url.protocol !== "http:" && url.protocol !== "https:") {
          return route.continue();
        }
        if (url.origin === allowedOrigin) {
          return route.continue();
        }
        if (await isBlockedUrl(url, lookupCache)) {
          console.error(`blocked request to ${url.origin} (resolves to a private or local address)`);
          return route.abort("blockedbyclient");
        }
        return route.continue();
      } catch {
        return route.abort("blockedbyclient");
      }
    });

    if (allowedOrigin) {
      await rejectBlockedRedirects(targetUrl, allowedOrigin, lookupCache);
    }
    await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 15000 });
    await page.waitForLoadState("networkidle", { timeout: 5000 }).catch(() => {});
    // Create the output directory only once navigation succeeds — placing this
    // after the SSRF guard and page.goto means SSRF-blocked runs exit before
    // touching the filesystem, which matters when running as a non-root user
    // without write access to the output directory's parent.
    fs.mkdirSync(require("path").dirname(outputPath), { recursive: true });
    await page.screenshot({
      path: outputPath,
      type: "jpeg",
      quality: 82,
      fullPage: false,
    });
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error(error && error.stack ? error.stack : String(error));
  process.exit(1);
});
