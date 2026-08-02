const fs = require("fs");
const path = require("path");
const net = require("net");
const { chromium } = require("playwright-core");
const {
  HOP_BY_HOP_HEADERS,
  internalRequest,
  isBlockedUrl,
  lookupPublicAddress,
  publicRequest,
  requestBody,
} = require("./network");

const [targetUrl, outputPath] = process.argv.slice(2);
if (!targetUrl || !outputPath) {
  console.error("usage: capture.js <url> <output-path>");
  process.exit(2);
}

const match = /^(\d+)x(\d+)$/.exec(process.env.HOSTLET_SCREENSHOT_SIZE || "1280x720");
const width = match ? Number(match[1]) : 1280;
const height = match ? Number(match[2]) : 720;
const deviceScaleFactor = 2;
const outputExtension = path.extname(outputPath).toLowerCase();
const outputFormat = (process.env.HOSTLET_SCREENSHOT_FORMAT || outputExtension.slice(1) || "webp")
  .toLowerCase()
  .replace("jpg", "jpeg");
if (!["jpeg", "webp"].includes(outputFormat)) {
  console.error("HOSTLET_SCREENSHOT_FORMAT/output extension must be jpeg, jpg, or webp");
  process.exit(2);
}
const screenshotQuality = Number(process.env.HOSTLET_SCREENSHOT_QUALITY) || 82;
const browserSmoke = process.env.HOSTLET_BROWSER_SMOKE === "1";
const BROWSER_SMOKE_SKIP = "HOSTLET_BROWSER_SMOKE_SKIPPED_NON_HTML";
const CLOUDFLARE_CHALLENGE_ERROR =
  "capture rejected: Cloudflare security challenge (cf-mitigated: challenge)";
const MAX_NAVIGATION_REDIRECTS = 10;

function navigationContentType(navigation, url) {
  const responseContentType = navigation?.headers()["content-type"] || "";
  if (responseContentType) return responseContentType;

  // Playwright does not expose response headers for data: URLs. Derive the
  // declared media type so the smoke fixtures—and any legitimate data URL
  // invocation—still distinguish HTML from non-HTML content.
  try {
    const parsed = new URL(url);
    if (parsed.protocol === "data:") {
      return parsed.pathname.split(",", 1)[0].split(";", 1)[0];
    }
  } catch {
    // page.goto will report an invalid target with the useful error later.
  }
  return "";
}

// Floor scales with deviceScaleFactor so a 2x capture (roughly 4x the pixels
// of 1x) isn't held to the same byte count as a 1x one. The base is the 1x
// value; env override applies before scaling so operators tune one number.
const MIN_BYTES_BASE_1X =
  Number(process.env.HOSTLET_SCREENSHOT_MIN_BYTES) || (outputFormat === "webp" ? 14000 : 35000);
const sizeFloorBytes = MIN_BYTES_BASE_1X * deviceScaleFactor;

function isRedirectStatus(status) {
  return [300, 301, 302, 303, 307, 308].includes(status);
}

function parseRouterPort(target) {
  const raw = process.env.HOSTLET_SCREENSHOT_ROUTER_PORT;
  if (raw === undefined || raw === "") return null;
  if (!target || (target.protocol !== "http:" && target.protocol !== "https:")) {
    throw new Error("HOSTLET_SCREENSHOT_ROUTER_PORT requires an http(s) target URL");
  }
  if (!/^\d{1,5}$/.test(raw)) {
    throw new Error("HOSTLET_SCREENSHOT_ROUTER_PORT must be a TCP port from 1 to 65535");
  }
  const port = Number(raw);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("HOSTLET_SCREENSHOT_ROUTER_PORT must be a TCP port from 1 to 65535");
  }
  if (!target.host || /[\r\n]/.test(target.host)) {
    throw new Error("canonical target host is not valid for internal routing");
  }
  if (target.username || target.password) {
    throw new Error("canonical target credentials are not supported with internal routing");
  }
  return {
    port,
    hostname: target.hostname.replace(/^\[|\]$/g, "").toLowerCase().replace(/\.$/, ""),
    hostHeader: target.host,
    canonicalOrigin: target.origin,
    canonicalProtocol: target.protocol,
  };
}

function isCanonicalHostname(url, router) {
  return (
    router &&
    url.hostname.replace(/^\[|\]$/g, "").toLowerCase().replace(/\.$/, "") ===
      router.hostname.replace(/\.$/, "")
  );
}

function isInternalRouterUrl(url, router) {
  if (!isCanonicalHostname(url, router) || url.protocol !== "http:") return false;
  return Number(url.port || 80) === router.port;
}

function internalUrlFor(url, router) {
  if (!router || !isCanonicalHostname(url, router)) {
    throw new Error("internal routing requested for a non-canonical host");
  }
  const hostname = net.isIP(router.hostname) === 6 ? `[${router.hostname}]` : router.hostname;
  const internal = new URL(`http://${hostname}:${router.port}`);
  internal.pathname = url.pathname;
  internal.search = url.search;
  return internal;
}

function normalizeInternalLocation(location, currentUrl, allowedOrigin, router) {
  const next = new URL(location, currentUrl);
  if (next.protocol !== "http:" && next.protocol !== "https:") {
    throw new Error(`blocked redirect to unsupported protocol ${next.protocol}`);
  }

  // The router may expose either a relative Location or an absolute HTTP URL
  // on its own listener port. Neither should leak the private listener URL to
  // Chromium: the page must retain its canonical HTTPS origin.
  if (isInternalRouterUrl(next, router)) {
    const canonical = new URL(allowedOrigin);
    canonical.pathname = next.pathname;
    canonical.search = next.search;
    canonical.hash = next.hash;
    return canonical;
  }
  if (router && isCanonicalHostname(next, router) && next.origin !== allowedOrigin) {
    throw new Error(
      `blocked canonical-host redirect to ${next.origin}; only the canonical origin is allowed`
    );
  }
  return next;
}

function locationHeader(headers) {
  if (!headers) return null;
  return headers.location || headers.Location || null;
}

function defaultCookiePath(pathname) {
  if (!pathname || !pathname.startsWith("/") || pathname === "/") return "/";
  const slash = pathname.lastIndexOf("/");
  return slash <= 0 ? "/" : pathname.slice(0, slash);
}

function parseSetCookie(value, logicalUrl) {
  const parts = String(value).split(";");
  const first = parts.shift() || "";
  const separator = first.indexOf("=");
  if (separator <= 0) return null;
  const name = first.slice(0, separator).trim();
  const cookieValue = first.slice(separator + 1).trim();
  if (!name || /[\x00-\x20(),\/:;<=>?@[\]{}]/.test(name)) return null;

  const cookie = {
    name,
    value: cookieValue,
    domain: logicalUrl.hostname,
    path: defaultCookiePath(logicalUrl.pathname),
  };
  let maxAge = null;
  for (const rawAttribute of parts) {
    const [rawName, ...rawValue] = rawAttribute.trim().split("=");
    const attribute = rawName.toLowerCase();
    const attributeValue = rawValue.join("=").trim();
    if (attribute === "domain" && attributeValue) {
      const domain = attributeValue.replace(/^\./, "").toLowerCase();
      const host = logicalUrl.hostname.toLowerCase();
      if (host !== domain && !host.endsWith(`.${domain}`)) return null;
      cookie.domain = attributeValue.toLowerCase();
    } else if (attribute === "path" && attributeValue.startsWith("/")) {
      cookie.path = attributeValue;
    } else if (attribute === "secure") {
      cookie.secure = true;
    } else if (attribute === "httponly") {
      cookie.httpOnly = true;
    } else if (attribute === "samesite") {
      const sameSite = attributeValue.toLowerCase();
      if (sameSite === "strict") cookie.sameSite = "Strict";
      else if (sameSite === "lax") cookie.sameSite = "Lax";
      else if (sameSite === "none") cookie.sameSite = "None";
    } else if (attribute === "max-age" && /^-?\d+$/.test(attributeValue)) {
      maxAge = Number(attributeValue);
    } else if (attribute === "expires" && attributeValue) {
      const expires = Date.parse(attributeValue);
      if (Number.isFinite(expires)) cookie.expires = Math.floor(expires / 1000);
    }
  }
  if (maxAge !== null) cookie.expires = Math.floor(Date.now() / 1000) + maxAge;
  return cookie;
}

async function applySetCookies(context, values, logicalUrl) {
  const cookies = values
    .map((value) => parseSetCookie(value, logicalUrl))
    .filter((cookie) => cookie !== null);
  if (cookies.length > 0) await context.addCookies(cookies);
}

// Requests fulfilled by the internal router never pass through Chromium's
// network stack, so Chromium does not get a chance to construct the Cookie
// header for a same-origin redirect hop. Rebuild it from the browser context
// only after a canonical/same-site request has been established; cross-site
// initiators retain their original browser-supplied header and never receive
// synthesized credentials.
async function headersWithBrowserCookies(context, url, requestHeaders = {}) {
  const headers = { ...requestHeaders };
  for (const name of Object.keys(headers)) {
    if (name.toLowerCase() === "cookie") delete headers[name];
  }
  const cookies = await context.cookies(url.href);
  if (cookies.length > 0) {
    headers.cookie = cookies.map((cookie) => `${cookie.name}=${cookie.value}`).join("; ");
  }
  return headers;
}

async function fetchInternalRoute(
  route,
  initialUrl,
  allowedOrigin,
  router,
  lookupCache,
  context
) {
  const request = route.request();
  const initialHeaders = { ...request.headers() };
  const initiatorOrigin = (() => {
    try {
      const frameUrl = request.frame()?.url();
      if (frameUrl) return new URL(frameUrl).origin;
    } catch {
      // Detached frames and early navigation requests have no usable origin.
    }
    return null;
  })();
  const isNavigation =
    typeof request.isNavigationRequest === "function" && request.isNavigationRequest();
  const canRebuildRedirectCookies =
    initialUrl.origin === allowedOrigin &&
    (initiatorOrigin === allowedOrigin || (initiatorOrigin === null && isNavigation));
  let current = initialUrl;
  let method = request.method();
  let postData = requestBody(request, initialHeaders);
  let headers = { ...initialHeaders, host: router.hostHeader };

  for (let hop = 0; hop < MAX_NAVIGATION_REDIRECTS; hop += 1) {
    // Keep the first-hop Cookie header exactly as Chromium supplied it. For
    // later canonical hops, context.cookies applies path/domain/expiry rules
    // after any Set-Cookie response has been installed.
    if (hop > 0 && canRebuildRedirectCookies) {
      headers = await headersWithBrowserCookies(context, current, headers);
    }
    const response = await internalRequest(current, router, {
      method,
      headers,
      body: postData,
    });
    const location = locationHeader(response.headers);
    if (!isRedirectStatus(response.status) || !location) {
      if (canRebuildRedirectCookies) {
        await applySetCookies(context, response.setCookies || [], current);
      }
      const responseHeaders = { ...response.headers };
      delete responseHeaders["set-cookie"];
      delete responseHeaders.connection;
      delete responseHeaders["keep-alive"];
      delete responseHeaders["transfer-encoding"];
      delete responseHeaders.upgrade;
      return {
        status: response.status,
        headers: responseHeaders,
        body: response.body,
      };
    }

    const next = normalizeInternalLocation(location, current, allowedOrigin, router);
    if (next.origin !== allowedOrigin) {
      if (await isBlockedUrl(next, lookupCache)) {
        throw new Error(`blocked request to ${next.origin} (resolves to a private or local address)`);
      }
      // Intentional public cross-origin redirects retain their normal browser
      // behavior, but the Location is still canonicalized if it came from the
      // internal listener.
      const responseHeaders = { ...response.headers, location: next.href };
      delete responseHeaders["set-cookie"];
      delete responseHeaders.connection;
      delete responseHeaders["keep-alive"];
      delete responseHeaders["transfer-encoding"];
      delete responseHeaders.upgrade;
      if (canRebuildRedirectCookies) {
        await applySetCookies(context, response.setCookies || [], current);
      }
      return { status: response.status, headers: responseHeaders, body: response.body };
    }

    if (hop === MAX_NAVIGATION_REDIRECTS - 1) {
      throw new Error(
        `too many redirects while fetching canonical subresource (limit ${MAX_NAVIGATION_REDIRECTS})`
      );
    }
    if (canRebuildRedirectCookies) {
      await applySetCookies(context, response.setCookies || [], current);
    }
    if ([301, 302, 303].includes(response.status) && !["GET", "HEAD"].includes(method)) {
      method = "GET";
      postData = undefined;
      headers = { ...headers };
      delete headers["content-length"];
      delete headers["content-type"];
    }
    current = next;
  }
  throw new Error("canonical subresource redirect resolution failed");
}

async function resolveNavigationTarget(startUrl, allowedOrigin, router, lookupCache, context) {
  let current = new URL(startUrl);
  for (let hop = 0; hop < MAX_NAVIGATION_REDIRECTS; hop += 1) {
    let response;
    if (router && current.origin === allowedOrigin) {
      // The direct Node request is deliberately manual: redirect targets are
      // validated one hop at a time and the canonical Host reaches Caddy.
      response = await internalRequest(current, router, {
        headers: await headersWithBrowserCookies(context, current),
      });
    } else if (current.origin !== allowedOrigin) {
      const address = await lookupPublicAddress(current, lookupCache);
      if (!address) {
        throw new Error(
          `blocked request to ${current.origin} (resolves to a private or local address)`
        );
      }
      response = await publicRequest(current, address);
    } else {
      const publicResponse = await fetch(current, { redirect: "manual" });
      response = {
        status: publicResponse.status,
        headers: Object.fromEntries(publicResponse.headers.entries()),
        setCookies:
          typeof publicResponse.headers.getSetCookie === "function"
            ? publicResponse.headers.getSetCookie()
            : [],
      };
    }
    if (!isRedirectStatus(response.status)) return current.href;

    await applySetCookies(context, response.setCookies || [], current);

    const location = locationHeader(response.headers);
    if (!location) {
      throw new Error(`redirect from ${current.origin} did not include a Location header`);
    }
    const next = normalizeInternalLocation(location, current, allowedOrigin, router);
    if (next.origin !== allowedOrigin) {
      if (await isBlockedUrl(next, lookupCache)) {
        throw new Error(`blocked request to ${next.origin} (resolves to a private or local address)`);
      }
      return next.href;
    }
    current = next;
  }
  throw new Error(
    `too many redirects while validating screenshot target (limit ${MAX_NAVIGATION_REDIRECTS})`
  );
}

// A capture is "visually ready" once authored CSS has plausibly applied and,
// if the page has <img> elements, at least one has actually decoded. This
// catches the class of bug where the page is DOM-complete and networkidle
// but the stylesheet/asset hadn't landed yet, producing an unstyled or
// blank-image screenshot that then gets stored permanently.
async function probeVisualReadiness(page) {
  return page.evaluate(async () => {
    const hasStylesheets = document.styleSheets.length > 0;
    const declaredStylesheets = document.querySelectorAll('link[rel~="stylesheet"], style').length;
    const hasInlineStyles = document.querySelector("[style]") !== null;
    const bodyFont = document.body
      ? window.getComputedStyle(document.body).fontFamily || ""
      : "";

    // Chromium resolves the browser's default serif font to a concrete font
    // name (e.g. "Times New Roman", or "Liberation Serif" where the Times
    // family is substituted), not the literal string "serif" — so the
    // default is measured from a pristine same-context reference frame with
    // no authored CSS rather than guessed as a hardcoded string. A measurement
    // failure (e.g. a page CSP blocking the reference frame) fails open —
    // it only skips the font check, it doesn't fail the probe.
    let defaultFont = null;
    try {
      const reference = document.createElement("iframe");
      reference.style.cssText = "position:absolute;width:0;height:0;border:0;visibility:hidden;";
      reference.srcdoc = "<!DOCTYPE html><html><body></body></html>";
      document.body.appendChild(reference);
      await new Promise((resolve, reject) => {
        reference.addEventListener("load", resolve, { once: true });
        setTimeout(() => reject(new Error("reference frame load timed out")), 2000);
      });
      defaultFont = reference.contentWindow.getComputedStyle(reference.contentDocument.body)
        .fontFamily;
      reference.remove();
    } catch {
      defaultFont = null;
    }

    const expectsAuthoredCss = declaredStylesheets > 0;
    const looksUnstyled =
      expectsAuthoredCss && !hasStylesheets && !hasInlineStyles && defaultFont !== null && bodyFont === defaultFont;
    if (looksUnstyled) return false;

    const images = Array.from(document.images || []);
    if (images.length > 0 && !images.some((img) => img.naturalWidth > 0)) {
      return false;
    }
    return true;
  });
}

async function ensureVisuallyReady(page) {
  if (await probeVisualReadiness(page)) return true;
  console.error("screenshot validity probe failed; waiting 5s and re-probing once");
  await page.waitForTimeout(5000);
  return probeVisualReadiness(page);
}

async function captureScreenshot(page, outputPath) {
  if (outputFormat === "webp") {
    const client = await page.context().newCDPSession(page);
    const capture = await client.send("Page.captureScreenshot", {
      format: "webp",
      quality: screenshotQuality,
      fromSurface: true,
      captureBeyondViewport: false,
    });
    const buffer = Buffer.from(capture.data, "base64");
    fs.writeFileSync(outputPath, buffer);
    return buffer;
  }
  return page.screenshot({
    path: outputPath,
    type: "jpeg",
    quality: screenshotQuality,
    fullPage: false,
  });
}

async function captureWithSizeFloor(page, outputPath) {
  let buffer = await captureScreenshot(page, outputPath);
  if (buffer.length >= sizeFloorBytes) return buffer;

  console.error(
    `screenshot capture too small (${buffer.length} bytes < ${sizeFloorBytes} byte floor); ` +
      "retrying once after an extra settle"
  );
  await page.waitForTimeout(3000);
  await page.waitForLoadState("networkidle", { timeout: 5000 }).catch(() => {});
  buffer = await captureScreenshot(page, outputPath);
  if (buffer.length < sizeFloorBytes) {
    const detail =
      `screenshot buffer ${buffer.length} bytes is below the ` +
      `${sizeFloorBytes} byte floor after retry`;
    if (browserSmoke) {
      throw new Error(`browser smoke rejected: page remained blank or near-blank; ${detail}`);
    }
    console.error(`capture warning: ${detail}; retaining the manual capture`);
  }
  return buffer;
}

function closeWebSocketRoute(websocket, code, reason) {
  try {
    const safeCode = Number.isInteger(code) && code >= 1000 && code <= 4999 ? code : 1011;
    return websocket.close({ code: safeCode, reason: String(reason || "internal WebSocket unavailable").slice(0, 123) });
  } catch {
    return Promise.resolve();
  }
}

async function main() {
  let target = null;
  try {
    const parsed = new URL(targetUrl);
    if (parsed.protocol === "http:" || parsed.protocol === "https:") target = parsed;
  } catch {
    // Let page.goto report malformed targets with its normal diagnostic.
  }
  const allowedOrigin = target ? target.origin : null;
  const router = parseRouterPort(target);
  const lookupCache = new Map();

  const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;
  const browser = await chromium.launch({
    headless: true,
    ...(executablePath ? { executablePath } : {}),
  });
  try {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor,
      serviceWorkers: "block",
    });
    const page = await context.newPage();
    const pageErrors = [];
    const criticalRequestFailures = [];
    const cspDiagnostics = [];
    if (browserSmoke) {
      page.on("pageerror", (error) => pageErrors.push(error.message || String(error)));
      page.on("requestfailed", (request) => {
        const resourceType = request.resourceType();
        let sameOrigin = false;
        try {
          sameOrigin = new URL(request.url()).origin === allowedOrigin;
        } catch {
          sameOrigin = false;
        }
        if (sameOrigin && ["document", "script", "stylesheet"].includes(resourceType)) {
          criticalRequestFailures.push(
            `${resourceType} ${request.url()}: ${request.failure()?.errorText || "request failed"}`
          );
        }
      });
      await page.addInitScript(() => {
        document.addEventListener("securitypolicyviolation", (event) => {
          console.debug(
            `HOSTLET_CSP_DIAGNOSTIC ${event.effectiveDirective} ${event.blockedURI || "unknown"}`
          );
        });
      });
      page.on("console", (message) => {
        if (message.type() === "debug" && message.text().startsWith("HOSTLET_CSP_DIAGNOSTIC ")) {
          cspDiagnostics.push(message.text());
        }
      });
    }

    await context.route("**/*", async (route) => {
      try {
        const url = new URL(route.request().url());
        if (url.protocol !== "http:" && url.protocol !== "https:") {
          return route.continue();
        }
        if (router && allowedOrigin && isCanonicalHostname(url, router) && url.origin !== allowedOrigin) {
          throw new Error(
            `blocked canonical-host request to ${url.href}; only the canonical origin may reach Chromium`
          );
        }
        if (router && allowedOrigin && url.origin === allowedOrigin) {
          const internalResponse = await fetchInternalRoute(
            route,
            url,
            allowedOrigin,
            router,
            lookupCache,
            context
          );
          return route.fulfill(internalResponse);
        }
        if (url.origin === allowedOrigin) {
          return route.continue();
        }
        const address = await lookupPublicAddress(url, lookupCache);
        if (!address) {
          throw new Error(
            `blocked request to ${url.origin} (resolves to a private or local address)`
          );
        }
        const request = route.request();
        const body = requestBody(request, request.headers());
        const response = await publicRequest(url, address, {
          method: request.method(),
          headers: request.headers(),
          body,
        });
        const responseHeaders = { ...response.headers };
        for (const name of HOP_BY_HOP_HEADERS) delete responseHeaders[name];
        return route.fulfill({
          status: response.status,
          headers: responseHeaders,
          body: response.body,
        });
      } catch (error) {
        // In particular, do not route.continue() after an internal router
        // failure: that would turn a transient private-router outage into an
        // unintended public-edge request.
        if (router) {
          console.error(`internal canonical request blocked: ${error.message || error}`);
        }
        return route.abort("blockedbyclient");
      }
    });

    if (router && typeof context.routeWebSocket !== "function") {
      throw new Error(
        "canonical WSS internal forwarding unavailable: Playwright WebSocket routing is not supported"
      );
    }
    if (typeof context.routeWebSocket === "function") {
      await context.routeWebSocket("**/*", async (websocket) => {
        try {
          if (router) {
            // WebSocketRoute exposes neither the browser handshake Origin nor
            // the resolved peer address. Any synthetic proxy would risk
            // granting canonical cookies/origin to an attacker iframe, and
            // connectToServer would re-resolve DNS. Keep router-mode captures
            // fail-closed until Playwright exposes those request semantics.
            return closeWebSocketRoute(
              websocket,
              1008,
              "WebSocket capture is disabled while internal routing is enabled"
            );
          }
          const url = new URL(websocket.url());
          if (url.protocol !== "ws:" && url.protocol !== "wss:") {
            console.error(`blocked WebSocket request to unsupported protocol ${url.protocol}`);
            return closeWebSocketRoute(websocket, 1002, "unsupported WebSocket protocol");
          }
          const websocketOrigin = url.protocol === "wss:" ? "https:" : "http:";
          const httpUrl = new URL(`${websocketOrigin}//${url.host}${url.pathname}${url.search}`);
          if (httpUrl.origin !== allowedOrigin && (await isBlockedUrl(httpUrl, lookupCache))) {
            console.error(`blocked request to ${url.origin} (resolves to a private or local address)`);
            return closeWebSocketRoute(websocket, 1008, "private or local WebSocket origin blocked");
          }
          websocket.connectToServer();
        } catch (error) {
          console.error(`blocked WebSocket request: ${error.message || error}`);
          return closeWebSocketRoute(websocket, 1008, "WebSocket request blocked");
        }
      });
    }

    const navigationUrl = allowedOrigin
      ? await resolveNavigationTarget(targetUrl, allowedOrigin, router, lookupCache, context)
      : targetUrl;
    const navigation = await page.goto(navigationUrl, {
      waitUntil: "domcontentloaded",
      timeout: 15000,
    });
    const navigationHeaders = navigation?.headers() || {};
    if (navigationHeaders["cf-mitigated"]?.trim().toLowerCase() === "challenge") {
      throw new Error(CLOUDFLARE_CHALLENGE_ERROR);
    }
    const navigationStatus = navigation?.status() || 0;
    if (navigationStatus >= 400) {
      throw new Error(`capture rejected: navigation returned HTTP ${navigationStatus}`);
    }
    if (browserSmoke) {
      const contentType = navigationContentType(navigation, navigationUrl);
      if (contentType && !contentType.toLowerCase().includes("text/html")) {
        console.log(`${BROWSER_SMOKE_SKIP} ${contentType}`);
        return;
      }
    }
    await page.waitForLoadState("networkidle", { timeout: 5000 }).catch(() => {});

    if (browserSmoke && pageErrors.length > 0) {
      throw new Error(`browser smoke rejected: uncaught page error: ${pageErrors.slice(0, 3).join(" | ")}`);
    }
    if (browserSmoke && criticalRequestFailures.length > 0) {
      throw new Error(
        `browser smoke rejected: critical same-origin resource failed: ${criticalRequestFailures.slice(0, 3).join(" | ")}`
      );
    }
    if (browserSmoke && cspDiagnostics.length > 0) {
      console.error(cspDiagnostics.slice(0, 5).join("\n"));
    }

    if (!(await ensureVisuallyReady(page))) {
      throw new Error(
        "capture rejected: page failed the visual-readiness probe after retry " +
          "(no stylesheets/font change and/or no decoded images)"
      );
    }

    // Create the output directory only once navigation succeeds — placing this
    // after the SSRF guard and page.goto means SSRF-blocked runs exit before
    // touching the filesystem, which matters when running as a non-root user
    // without write access to the output directory's parent.
    fs.mkdirSync(require("path").dirname(outputPath), { recursive: true });
    await captureWithSizeFloor(page, outputPath);
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error(error && error.stack ? error.stack : String(error));
  process.exit(1);
});
