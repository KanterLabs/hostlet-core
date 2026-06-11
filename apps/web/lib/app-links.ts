// Shared URL/host helpers for the apps list and app detail pages.
//
// These compute the "visit" link and label for an app based on whether it is
// publicly exposed (use its domain) or private (use the machine host + the
// published port). They also normalise localhost domains to the page host so
// links work when the dashboard is reached over the network.
//
// `displayDomain`, `privateAppHost` and `appVisitHref` reference `window`; every
// such access is guarded with `typeof window` so these helpers stay safe to
// import from both client and server components.

export type VisitableServer = {
  publicIp?: string | null;
} | null | undefined;

export type VisitableApp = {
  currentDeploymentId?: string | null;
  publicExposure?: boolean | null;
  domain: string;
  currentDeployment?: { publishedPort?: number | null } | null;
  server?: VisitableServer;
};

export function privateAppHost(app: VisitableApp) {
  const host = app.server?.publicIp?.trim();
  if (host && host !== "127.0.0.1" && host !== "localhost" && host !== "0.0.0.0") return host;
  if (typeof window !== "undefined") return window.location.hostname;
  return host || null;
}

export function displayDomain(domain: string) {
  if (!domain || typeof window === "undefined") return domain;
  try {
    const withProtocol = domain.startsWith("http://") || domain.startsWith("https://") ? domain : `http://${domain}`;
    const url = new URL(withProtocol);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1") {
      url.hostname = window.location.hostname;
      return url.host + url.pathname.replace(/\/$/, "");
    }
  } catch {
    return domain;
  }
  return domain;
}

export function appVisitHref(app?: VisitableApp | null) {
  if (!app?.currentDeploymentId) return null;
  if (!app.publicExposure) {
    const port = app.currentDeployment?.publishedPort;
    const host = privateAppHost(app);
    return port && host ? `http://${host}:${port}` : null;
  }
  const display = displayDomain(app.domain);
  if (!display) return null;
  if (display.startsWith("http://") || display.startsWith("https://")) return display;
  try {
    const url = new URL(`http://${display}`);
    if (url.hostname === "localhost" || url.hostname === "127.0.0.1" || /^[\d.]+$/.test(url.hostname)) {
      return `http://${display}`;
    }
  } catch {
    return null;
  }
  return `https://${display}`;
}

export function appVisitLabel(app: VisitableApp) {
  if (app.publicExposure) return displayDomain(app.domain) || "No public URL";
  const port = app.currentDeployment?.publishedPort;
  const host = privateAppHost(app);
  return port && host ? `${host}:${port}` : "Deploy to assign a private port";
}
