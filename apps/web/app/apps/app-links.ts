// Re-export shim. The canonical implementations now live in `@/lib/app-links`
// (domain/href/label/host helpers) and `@/lib/app-status` (deploy predicates),
// so cloud web — which overrides this route file — can import them from `lib/`.
// Kept as a shim so existing `./app-links` import sites keep working.

export type { VisitableServer, VisitableApp } from "@/lib/app-links";
export { privateAppHost, displayDomain, appVisitHref, appVisitLabel } from "@/lib/app-links";
export { isActiveDeploy, shortSha } from "@/lib/app-status";
