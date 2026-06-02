import { apiUrl } from "./api";

export type WebhookReadiness = {
  apiBaseUrl: string;
  webhookUrl: string;
  canReceiveGitHub: boolean;
  reason: string;
};

export function webhookReadiness(): WebhookReadiness {
  const webhookBaseUrl = safeWebhookUrl();

  try {
    const url = new URL(webhookBaseUrl);
    // Only build the webhook URL once the base is known to be a valid URL, so
    // the success path never derives a path from a malformed base.
    const apiBaseUrl = webhookBaseUrl.replace(/\/$/, "");
    const webhookUrl = `${apiBaseUrl}/webhooks/github`;
    const privateHost = isPrivateHost(url.hostname);
    const https = url.protocol === "https:";
    const issues = [
      !https ? "the API URL is not HTTPS" : "",
      privateHost ? "the host is local or private" : "",
    ].filter(Boolean);

    return {
      apiBaseUrl,
      webhookUrl,
      canReceiveGitHub: https && !privateHost,
      reason: issues.length ? issues.join(" and ") : "public HTTPS webhook URL",
    };
  } catch {
    return {
      apiBaseUrl: webhookBaseUrl,
      webhookUrl: `${webhookBaseUrl.replace(/\/$/, "")}/webhooks/github`,
      canReceiveGitHub: false,
      reason: "the webhook URL is not a valid absolute URL",
    };
  }
}

function safeWebhookUrl() {
  const configured = process.env.NEXT_PUBLIC_WEBHOOK_URL?.trim() || process.env.PUBLIC_WEBHOOK_URL?.trim();
  return configured || safeApiUrl();
}

function safeApiUrl() {
  try {
    return apiUrl();
  } catch {
    return process.env.NEXT_PUBLIC_API_URL || process.env.PUBLIC_API_URL || "http://localhost:8080";
  }
}

function isPrivateHost(hostname: string) {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, "");

  // Loopback / unspecified addresses and hostnames.
  if (host === "localhost" || host === "0.0.0.0" || host === "::1") return true;

  // Special-use mDNS / local-network suffixes.
  if (host.endsWith(".local") || host.endsWith(".lan") || host.endsWith(".home.arpa")) return true;

  // IPv4 loopback (127.0.0.0/8) and link-local (169.254.0.0/16).
  if (host.startsWith("127.") || host.startsWith("169.254.")) return true;

  // IPv4 private ranges (RFC 1918): 10.0.0.0/8, 192.168.0.0/16, 172.16.0.0/12.
  if (host.startsWith("10.")) return true;
  if (host.startsWith("192.168.")) return true;
  if (/^172\.(1[6-9]|2\d|3[0-1])\./.test(host)) return true;

  // IPv6 unique-local (fc00::/7 -> fc/fd) and link-local (fe80::/10).
  if (host.startsWith("fc") || host.startsWith("fd") || host.startsWith("fe80:")) return true;

  return false;
}
