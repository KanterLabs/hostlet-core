# Hostlet Roadmap: 0.4.1 through 0.8.0

## Summary

Use a balanced roadmap: keep Hostlet Cloud moving toward a real paid beta while preserving self-hosted quality. The work originally split across `0.5.0` and `0.6.0` is being consolidated into one `0.5.0` release, with build performance, auto Docker packaging, and cloud beta UX landing together.

## Milestones

- `0.4.1`: Managed cloud auto-redeploy.
  - Shipped: cloud apps default to public Hostlet Cloud URLs and auto-redeploy on push.
  - Follow-up: keep the implementation report as the source for what changed.

- `0.4.2`: Release and production cleanup patch.
  - Fix release manifest image digest capture so digests are real, not empty SHA values.
  - Standardize GCP production deployment docs around `/srv/hostlet`, project `infra`, and tagged `vX.Y.Z` images.
  - Update public/private infra docs so GCP production is not confused with homelab `hostlet`.
  - Add a deploy checklist that verifies authenticated `/api/system/version`.

- `0.5.0`: Build performance, auto Docker packaging, and cloud beta UX.
  - Add app packaging strategy: `auto`, `dockerfile`, and `generated`.
  - Keep `auto` backward-compatible: use a repo Dockerfile when present, otherwise use Hostlet-generated Docker.
  - Let users explicitly choose Hostlet optimized generated Docker even when a repo Dockerfile exists.
  - Add BuildKit/local cache support on the current GCP VM builder before introducing dedicated builder VMs.
  - Replace the current single-stage generated Node image with framework-specific multi-stage Dockerfiles.
  - Track and surface build duration, detected framework, package manager, packaging strategy, and final image size.
  - Target major final image-size reductions for generated apps, especially the current Next.js/pnpm path that produced a 1.51 GB image.
  - Keep critical ops hardening in scope: production health checks, release smoke checks, backup/restore confidence, and update clarity.

- `0.6.0`: Cloud beta UX. Consolidated into `0.5.0` for this release.
  - Tighten GitHub App onboarding, billing setup, create-app flow, deploy logs, failed deploy recovery, and empty states.
  - Add cloud app lifecycle polish: redeploy status, latest push metadata, retry affordances, and clearer plan-limit handling.
  - Use 0.5 packaging metadata in the UI: detected framework, package manager, build duration, and image size.
  - Keep cloud runtime constrained to single-service Dockerfile/generated web apps.

- `0.7.0`: Live balanced beta.
  - Start remote self-hosted VPS agent registration and deployment.
  - Add server inventory, agent install tokens, heartbeat state, and per-server deploy targeting.
  - Preserve single-machine local deploy behavior as the default path.
  - Use 0.5 build metrics to decide whether dedicated cloud builder VMs are needed.

- `0.8.0`: Live balanced beta.
  - Enable real paid Hostlet Cloud beta with Stripe live mode, billing portal, cancellation/update flows, and tight plan limits.
  - Add custom domains for Hostlet Cloud if Cloudflare automation and ownership verification are ready.
  - Keep self-hosted production-grade: reliable updates, backups, remote agents, and clear rollback paths.

## Interface And Data Changes

- Add operator/diagnostic API fields for deployed image tag, revision, release manifest digest status, and runtime mode.
- Add `apps.packaging_strategy` in `0.5.0`, defaulting existing apps to `auto`.
- Add deployment runtime metadata for packaging strategy, detected framework, package manager, final image size, and build duration.
- Add remote-agent registration tables and APIs in `0.7.0`, gated to self-hosted mode first.
- Add cloud custom-domain tables/APIs only in `0.8.0`, with ownership verification before routing.
- Do not add customer-controlled cloud CPU/RAM, cloud Compose, or managed databases before `0.8.0`.

## Test Plan

- CI: keep `cargo fmt`, workspace tests, clippy, web lint/build, Playwright, responsive QA, Compose config checks, and image builds.
- Agent: test framework detection, package manager detection, packaging strategy selection, generated Dockerfile shapes, BuildKit command construction, image-size parsing, and build-duration metadata.
- Docker fixtures: verify at least one generated Vite/static app and one generated Next.js app build successfully and produce smaller final images than the current single-stage Node baseline.
- Release: add manifest-digest verification and tag/version consistency checks.
- Cloud E2E: cover signup gates, billing active/inactive, app limits, create/deploy, auto-redeploy, and route health.
- Self-hosted E2E: cover local app deploy, rollback, backups, restore preflight, and remote-agent registration when introduced.

## Assumptions

- `0.8.0` target is a balanced product with real paid Hostlet Cloud beta.
- `0.5.0` prioritizes build performance, generated Docker quality, and app packaging UX before larger product expansion.
- JavaScript web apps are the first generated-runtime expansion: Next.js, Vite, Astro, Nuxt, Remix, SvelteKit, static SPAs, and generic Node.
- Python, Go, Rust, managed databases, and cloud Compose remain out of scope for the first build performance overhaul.
- Existing apps stay compatible through the `auto` packaging default.
- The first build backend target remains the current GCP VM with BuildKit/local cache, not dedicated builder infrastructure.
- Cloud remains private/beta until live billing, deployment reliability, and production runbooks are strong enough.
- No tracked docs should contain private production inventory, secret values, or raw env contents.
