export function apiUrl() {
  const configured = process.env.NEXT_PUBLIC_API_URL || "http://localhost:8080";
  if (typeof window === "undefined") return configured;

  const url = new URL(configured);
  const pageHost = window.location.hostname;
  if ((url.hostname === "localhost" || url.hostname === "127.0.0.1") && pageHost !== "localhost" && pageHost !== "127.0.0.1") {
    url.hostname = pageHost;
  }
  return url.toString().replace(/\/$/, "");
}

export const API_URL = process.env.NEXT_PUBLIC_API_URL || "http://localhost:8080";

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);
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
  if (!res.ok) throw new Error(await res.text());
  if (res.status === 204) return undefined as T;
  return res.json();
}
