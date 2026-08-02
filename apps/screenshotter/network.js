const dns = require("dns").promises;
const http = require("http");
const https = require("https");
const net = require("net");

const MAX_INTERNAL_RESPONSE_BYTES = 32 * 1024 * 1024;
const MAX_ROUTED_REQUEST_BYTES = 16 * 1024 * 1024;

function isCanonicalHostname(url, router) {
  return (
    router &&
    url.hostname.replace(/^\[|\]$/g, "").toLowerCase().replace(/\.$/, "") ===
      router.hostname.replace(/\.$/, "")
  );
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

async function lookupPublicAddress(url, lookupCache) {
  const host = url.hostname.replace(/^\[|\]$/g, "");
  if (net.isIP(host)) {
    return isBlockedIp(host) ? null : { address: host, family: net.isIP(host) };
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
  if (addresses.length === 0 || addresses.some((entry) => isBlockedIp(entry.address))) {
    return null;
  }
  // Pin the request to the address selected from this lookup. Chromium's own
  // resolver is not used for requests fulfilled by the Node proxy, avoiding a
  // DNS-rebinding window between validation and connection.
  return addresses[0];
}

async function isBlockedUrl(url, lookupCache) {
  return (await lookupPublicAddress(url, lookupCache)) === null;
}

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

function withoutHopByHopHeaders(requestHeaders) {
  const headers = { ...requestHeaders };
  for (const name of Object.keys(headers)) {
    if (HOP_BY_HOP_HEADERS.has(name.toLowerCase())) delete headers[name];
  }
  return headers;
}

function deleteHeader(headers, name) {
  for (const key of Object.keys(headers)) {
    if (key.toLowerCase() === name.toLowerCase()) delete headers[key];
  }
}

function setHeader(headers, name, value) {
  deleteHeader(headers, name);
  headers[name] = value;
}

function responseHeaders(response) {
  const headers = {};
  for (const [name, value] of Object.entries(response.headers)) {
    if (value === undefined) continue;
    headers[name.toLowerCase()] = Array.isArray(value) ? value.join(", ") : String(value);
  }
  return headers;
}

function rejectEncodedResponse(response, label) {
  const contentEncoding = String(response.headers["content-encoding"] || "").trim().toLowerCase();
  if (!contentEncoding || contentEncoding === "identity") return false;
  response.on("error", () => {});
  response.resume();
  response.destroy();
  return new Error(
    `${label} returned a compressed response (${contentEncoding}); refusing to forward it`
  );
}

function requestBody(request, requestHeaders) {
  const declaredLength = Number(requestHeaders["content-length"] || requestHeaders["Content-Length"]);
  if (Number.isFinite(declaredLength) && declaredLength > MAX_ROUTED_REQUEST_BYTES) {
    throw new Error(`routed request body exceeds ${MAX_ROUTED_REQUEST_BYTES} byte limit`);
  }
  const body = request.postDataBuffer();
  if (body && body.length > MAX_ROUTED_REQUEST_BYTES) {
    throw new Error(`routed request body exceeds ${MAX_ROUTED_REQUEST_BYTES} byte limit`);
  }
  return body;
}

function internalRequest(
  url,
  router,
  { method = "GET", headers: requestHeaders = {}, body = null } = {}
) {
  if (body && body.length > MAX_ROUTED_REQUEST_BYTES) {
    return Promise.reject(
      new Error(`routed request body exceeds ${MAX_ROUTED_REQUEST_BYTES} byte limit`)
    );
  }
  const target = internalUrlFor(url, router);
  return new Promise((resolve, reject) => {
    const headers = withoutHopByHopHeaders(requestHeaders);
    deleteHeader(headers, "host");
    if (body) setHeader(headers, "content-length", String(body.length));
    else deleteHeader(headers, "content-length");
    // Ask the router for an uncompressed response. Forwarding a compressed
    // body to Chromium would make the byte cap apply only to compressed bytes
    // and permit a decompression bomb in the browser process.
    setHeader(headers, "accept-encoding", "identity");
    setHeader(headers, "host", router.hostHeader);
    const request = require("http").request(
      {
        protocol: target.protocol,
        hostname: target.hostname,
        port: target.port,
        path: `${target.pathname}${target.search}`,
        method,
        // Host is intentionally explicit and validated from the canonical
        // CLI URL. Do not let the router listener port become the Host value.
        headers,
        timeout: 15000,
      },
      (response) => {
        const encodingError = rejectEncodedResponse(response, "internal router");
        if (encodingError) {
          reject(encodingError);
          return;
        }
        const chunks = [];
        let totalBytes = 0;
        response.on("data", (chunk) => {
          totalBytes += chunk.length;
          if (totalBytes > MAX_INTERNAL_RESPONSE_BYTES) {
            response.destroy(
              new Error(
                `internal router response exceeded ${MAX_INTERNAL_RESPONSE_BYTES} byte limit`
              )
            );
            return;
          }
          chunks.push(chunk);
        });
        response.on("error", reject);
        response.on("end", () => {
          resolve({
            status: response.statusCode || 0,
            headers: responseHeaders(response),
            setCookies: response.headers["set-cookie"] || [],
            body: Buffer.concat(chunks),
          });
        });
      }
    );
    request.on("timeout", () => request.destroy(new Error("internal router request timed out")));
    request.on("error", reject);
    if (body) request.write(body);
    request.end();
  });
}

function publicRequest(
  url,
  address,
  { method = "GET", headers: requestHeaders = {}, body = null } = {}
) {
  if (body && body.length > MAX_ROUTED_REQUEST_BYTES) {
    return Promise.reject(
      new Error(`routed request body exceeds ${MAX_ROUTED_REQUEST_BYTES} byte limit`)
    );
  }
  return new Promise((resolve, reject) => {
    const headers = withoutHopByHopHeaders(requestHeaders);
    const hostHeader = Object.entries(headers).find(([name]) => name.toLowerCase() === "host")?.[1];
    if (body) setHeader(headers, "content-length", String(body.length));
    else deleteHeader(headers, "content-length");
    setHeader(headers, "accept-encoding", "identity");
    setHeader(headers, "host", hostHeader || url.host);
    const transport = url.protocol === "https:" ? https : http;
    const request = transport.request(
      {
        protocol: url.protocol,
        hostname: url.hostname,
        port: url.port || (url.protocol === "https:" ? 443 : 80),
        path: `${url.pathname}${url.search}`,
        method,
        headers,
        // The socket is pinned to the address validated by lookupPublicAddress,
        // while hostname/servername retain the public URL's Host and TLS SNI.
        lookup: (_hostname, _options, callback) =>
          callback(null, address.address, address.family),
        ...(url.protocol === "https:" ? { servername: url.hostname } : {}),
        timeout: 15000,
      },
      (response) => {
        const encodingError = rejectEncodedResponse(response, "public origin");
        if (encodingError) {
          reject(encodingError);
          return;
        }
        const chunks = [];
        let totalBytes = 0;
        response.on("data", (chunk) => {
          totalBytes += chunk.length;
          if (totalBytes > MAX_INTERNAL_RESPONSE_BYTES) {
            response.destroy(
              new Error(`public response exceeded ${MAX_INTERNAL_RESPONSE_BYTES} byte limit`)
            );
            return;
          }
          chunks.push(chunk);
        });
        response.on("error", reject);
        response.on("end", () => {
          resolve({
            status: response.statusCode || 0,
            headers: responseHeaders(response),
            setCookies: response.headers["set-cookie"] || [],
            body: Buffer.concat(chunks),
          });
        });
      }
    );
    request.on("timeout", () => request.destroy(new Error("public request timed out")));
    request.on("error", reject);
    if (body) request.write(body);
    request.end();
  });
}

module.exports = {
  HOP_BY_HOP_HEADERS,
  internalRequest,
  isBlockedUrl,
  lookupPublicAddress,
  publicRequest,
  requestBody,
};
