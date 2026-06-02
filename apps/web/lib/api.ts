// Default API base used when neither NEXT_PUBLIC_API_URL nor PUBLIC_API_URL is set.
const DEFAULT_API_URL = "http://localhost:8080";

// Port the dev/staging reverse proxy listens on; when the page is served from it
// the API lives on the same origin, so no hostname rewrite is needed.
const PROXY_PORT = "18080";

// Abort an in-flight request after this many ms so a hung API never hangs the UI.
const REQUEST_TIMEOUT_MS = 5000;

// API base for server-side use: prefers a trimmed NEXT_PUBLIC_API_URL, then
// PUBLIC_API_URL, then the local default.
function serverApiUrl() {
  return process.env.NEXT_PUBLIC_API_URL?.trim() || process.env.PUBLIC_API_URL || DEFAULT_API_URL;
}

function isLocalHostname(hostname: string) {
  return hostname === "localhost" || hostname === "127.0.0.1";
}

export function apiUrl() {
  // On the server there is no window; use the configured base directly.
  if (typeof window === "undefined") return serverApiUrl();

  const configured = process.env.NEXT_PUBLIC_API_URL?.trim();

  // No explicit API URL: derive one from the current page location.
  if (!configured) {
    // Served over HTTPS or via the dev proxy -> API shares this origin.
    const sameOrigin = window.location.protocol === "https:" || window.location.port === PROXY_PORT;
    if (sameOrigin) return window.location.origin;
    // Plain HTTP dev: API runs on the standard port of the same host.
    return `${window.location.protocol}//${window.location.hostname}:8080`;
  }

  // A localhost-configured URL viewed from a non-localhost page (e.g. a LAN IP)
  // would be unreachable, so rewrite the host to match the page.
  const url = new URL(configured);
  const pageHost = window.location.hostname;
  if (isLocalHostname(url.hostname) && !isLocalHostname(pageHost)) {
    url.hostname = pageHost;
  }
  return url.toString().replace(/\/$/, "");
}

export const API_URL = process.env.NEXT_PUBLIC_API_URL || process.env.PUBLIC_API_URL || DEFAULT_API_URL;

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  const method = (init.method || "GET").toUpperCase();
  const headers = {
    ...(init.body ? { "Content-Type": "application/json" } : {}),
    ...(!["GET", "HEAD"].includes(method) ? { "X-Hostlet-CSRF": "1" } : {}),
    ...(init.headers || {}),
  };
  const res = await fetch(`${apiUrl()}${path}`, {
    ...init,
    credentials: "include",
    signal: controller.signal,
    headers,
    cache: "no-store",
  }).finally(() => clearTimeout(timeout));
  if (res.status === 401 && typeof window !== "undefined" && window.location.pathname !== "/login") {
    window.location.assign("/login");
    throw new Error("Sign in required.");
  }
  if (!res.ok) throw new Error(await readableError(res));
  if (res.status === 204) return undefined as T;
  return res.json();
}

async function readableError(res: Response) {
  const text = await res.text();
  const contentType = res.headers.get("content-type") || "";
  if (contentType.includes("text/html") || /^\s*<!doctype html/i.test(text) || /^\s*<html/i.test(text)) {
    return `Request failed with ${res.status} ${res.statusText || "error"}. The Hostlet API returned an HTML error page.`;
  }
  return text || `Request failed with ${res.status} ${res.statusText || "error"}.`;
}
