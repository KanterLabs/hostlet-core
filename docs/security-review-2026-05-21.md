# Hostlet Security Review - 2026-05-21

## Scope

Reviewed the control-plane API, GitHub OAuth/webhooks, frontend auth gate, deployment agent, generated Docker build/runtime flow, Caddy routing, Docker Compose infrastructure, and dependency audit output.

## Fixed In This Pass

- Replaced broad credentialed LAN/Tailscale CORS with exact configured web origins.
- Added server-side Origin plus `X-Hostlet-CSRF` checks for mutating browser API routes.
- Added frame-ancestor, frame-options, content-type, referrer, and permissions headers on API and web responses.
- Removed the frontend `localStorage` unlock bypass. Unlock state now comes from signed server cookies.
- Required the control-plane unlock cookie before the first GitHub connection.
- Replaced callback-based OAuth with GitHub Device Flow so self-hosted installs do not depend on redirect URI matching.
- Made first-run password setup atomic instead of check-then-upsert.
- Added optional `HOSTLET_SETUP_TOKEN` support for secure-mode first-run setup.
- Added a uniqueness migration and upsert for stored GitHub account tokens to prevent duplicate stale token rows.
- Redacted Docker `-e KEY=value` values before command lines are stored in deployment logs.
- Moved generated Dockerfiles out of repo-controlled `.hostlet` paths.
- Canonicalized app root directories so symlinks cannot escape the repository checkout.
- Keyed agent checkouts, images, containers, and Caddy route files by app ID instead of mutable display names.
- Removed stale Caddy route snippets for the same domain when writing a new route.
- Recorded published host ports for deployments and used them for rollback routing.
- Made local rollback actually update and reload the local Caddy route.
- Ignored duplicate GitHub webhook delivery IDs before deploying and only accepted 40-character commit SHAs.
- Had the agent check out and verify the signed webhook commit instead of blindly deploying branch HEAD.
- Added deployment command timeouts, basic status validation, log stream validation, log line truncation, and per-deployment log caps.
- Bound Postgres to loopback in Compose and removed the API container's Docker socket mount.
- Stopped passing the Cloudflare tunnel token on the container command line.
- Switched web dependency installs in Compose to frozen lockfile mode.
- Fixed the web PostCSS audit by forcing Next's PostCSS transitive dependency to the patched version.

## Remaining High-Risk Items

- The local agent still needs Docker socket access and host networking to build/run apps on this machine. Treat the agent as privileged host control. Production should isolate it with rootless Docker, a constrained Docker proxy, or a dedicated build host.
- The API still exposes the control plane on `8080` for LAN web access. Keep `PUBLIC_WEB_URL` and `HOSTLET_ALLOWED_WEB_ORIGINS` exact, and do not expose the API directly to the public internet without an auth proxy or VPN.
- Deleting an app now enqueues local agent teardown for managed containers, images, app data volume, and routes before removing database records. Delete finalization still needs durable API-restart reconciliation.
- Agent events are still bearer-token authenticated. Stronger production hardening should add per-job nonces/signatures and stricter status transition checks.
- Job signing still uses a shared secret. A multi-server production version should use per-server keys and expiry-bound signed envelopes.
- Compose still uses dev-oriented floating images. Production should use built, pinned images and separate dev/prod Compose profiles.

## Audit Notes

- `pnpm --dir apps/web audit --prod` is clean after the PostCSS override.
- Raw `cargo audit` reports `RUSTSEC-2023-0071` through SQLx's optional MySQL/RSA lockfile entries. `cargo tree -p hostlet-api -e features` does not include `sqlx-mysql`; `cargo audit --ignore RUSTSEC-2023-0071` is clean. This should stay documented until SQLx/cargo-audit can avoid the inactive optional dependency warning or the SQLx dependency is reworked.
