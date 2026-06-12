const fs = require("fs");
const dns = require("dns").promises;
const net = require("net");
const { chromium } = require("playwright");

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
    const mapped = /^::ffff:(\d+\.\d+\.\d+\.\d+)$/.exec(lower);
    if (mapped) return isBlockedIp(mapped[1]);
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

async function main() {
  fs.mkdirSync(require("path").dirname(outputPath), { recursive: true });
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({
      viewport: { width, height },
      deviceScaleFactor: 1,
      serviceWorkers: "block",
    });

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
    await page.route("**/*", async (route) => {
      try {
        const url = new URL(route.request().url());
        if (url.protocol !== "http:" && url.protocol !== "https:") {
          return route.continue();
        }
        if (url.origin === allowedOrigin) {
          return route.continue();
        }
        const host = url.hostname.replace(/^\[|\]$/g, "");
        let blocked;
        if (net.isIP(host)) {
          blocked = isBlockedIp(host);
        } else {
          let addresses = lookupCache.get(host);
          if (!addresses) {
            try {
              addresses = await dns.lookup(host, { all: true, verbatim: true });
            } catch {
              addresses = [];
            }
            lookupCache.set(host, addresses);
          }
          blocked =
            addresses.length === 0 || addresses.some((entry) => isBlockedIp(entry.address));
        }
        if (blocked) {
          console.error(`blocked request to ${url.origin} (resolves to a private or local address)`);
          return route.abort("blockedbyclient");
        }
        return route.continue();
      } catch {
        return route.abort("blockedbyclient");
      }
    });

    await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 15000 });
    await page.waitForLoadState("networkidle", { timeout: 5000 }).catch(() => {});
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
