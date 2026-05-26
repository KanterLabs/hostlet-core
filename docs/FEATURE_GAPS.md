# Current Capability and Gap Report

Date: 2026-05-26

This document describes the current 0.4.0 package. Hostlet has two product paths:

- **Self-hosted Hostlet**: single-machine, single-owner homelab deployment with local Docker, Caddy, optional Cloudflare Tunnel, GitHub Device Flow, and local app operations.
- **Hostlet Cloud private beta**: hosted control plane at `hostlet.cloud` with managed worker compute, `*.hostlet.cloud` app URLs, GitHub App repository access, and Stripe sandbox subscription gates.

Do not describe 0.4.0 as a mature multi-user production SaaS, a remote self-hosted VPS fleet manager, or a live Stripe production service.

## Current Capabilities

- First-run setup token, control-plane password, unlock gate, logout, signed cookies, and session revocation.
- Self-hosted GitHub Device Flow login with optional GitHub login allowlist.
- Cloud GitHub OAuth login plus GitHub App installation state validation.
- Encrypted GitHub tokens and app environment variables.
- GitHub repository listing, repo inspection, private repository deploys, and log redaction.
- Local/managed deployment agent connected over authenticated WebSocket/events.
- Signed deployment jobs, job lease renewal, live deployment logs, stored deployment logs, and deployment log WebSocket reconnect UI.
- Dockerfile-based deploys and generated Dockerfiles for common Node apps.
- Self-hosted constrained Docker Compose support for one public web service and private supporting services.
- Cloud single-service app support with Compose, custom domains, public/private toggles, auto-deploy toggles, and arbitrary CPU/RAM edits rejected.
- Configurable root directory, install/build/start commands, container port, and health path.
- Self-hosted CPU/memory limits, public URL publishing, auto-redeploy controls, and rollback for single-service apps.
- Cloud entitlement gates for app count and starter resource limits.
- Per-app Cloudflare DNS publication under the configured base domain, with ownership tracking in `app_public_dns_records`.
- GitHub push webhook handling with signature verification, delivery dedupe, exact commit deploys, and per-app webhook status.
- Stripe sandbox checkout, subscription webhook processing, timestamp-tolerant signature validation, and webhook event dedupe.
- Runtime health checks with `healthy`, `degraded`, `unhealthy`, and `unknown` states.
- App health history, health filters, dashboard health counts, manual health check, and manual current-container restart.
- Loopback-only raw app port binding and Caddy route reload restoration on failure.
- Hostlet update detection, release metadata, `hostlet update check`, `hostlet update --dry-run`, `hostlet update`, and `hostlet update rollback`.
- `hostlet status`, `hostlet doctor`, production Dockerfiles, production Compose, CLI setup wizard, backup, and restore.
- Security documentation, threat model, ownership review, and expanded 0.4.0 validation gates.

## Explicit Scope Limits

- Self-hosted remote VPS agents are disabled. Self-hosted UI, API, Postgres, Caddy, local agent, and deployed app containers run on the same host.
- Hostlet Cloud 0.4.0 is a private beta on managed Hostlet infrastructure, not a general multi-worker cloud platform.
- Stripe remains sandbox-only for 0.4.0.
- Cloud custom domains, Compose support, managed databases, persistent disk upsells, multi-worker scheduling, live Stripe mode, and arbitrary customer resource edits are deferred.
- Compose rollback is disabled for 0.4.0. Redeploy the desired revision instead.
- Single-service apps receive a Hostlet-managed `/data` volume. Compose apps keep their declared named volumes; Hostlet does not inject `/data` into arbitrary Compose services.
- Automatic self-healing policies are not enabled. The owner/customer can manually check, restart, redeploy, or rollback where supported.
- `hostlet update rollback` restores the previous CLI binary and saved Compose files, then restarts services. Database rollback remains manual from backup.
- Audit UI, RBAC, team support, active-session UI, app-level permissions, and tenant-admin features are not implemented.
- Rate limits are in-memory and reset when the API restarts.
- Dependency/image scanning, release signing, SBOMs, and provenance attestations are not enforced.
- Backups are local scripts only. Scheduled backup, off-host backup, and clean-machine restore validation remain operator work.
- Log, image, failed-container, webhook, agent-job, and resource-snapshot retention policies are limited and need more automation.

## Remaining 0.4.0 Validation Focus

- Add and run broader Rust tests for unpaid cloud users, missing GitHub App install, inactive subscriptions, revoked sessions, and cross-user isolation.
- Add Stripe webhook tests for duplicate events, missing metadata, checkout completion, subscription updates, cancellation, and deletion.
- Add Postgres migration/API smoke coverage in CI for cloud mode.
- Add Playwright or equivalent web smoke coverage for login/setup states, create app, deployment logs, mode-specific controls, disabled reasons, and production security headers.
- Run responsive QA at 320, 375, 768, 1024, and desktop widths.
- Complete manual `hostlet.cloud` paid-deploy validation with sandbox Stripe and a real GitHub user.

## Later Reliability Track

- Move deploy, rollback, delete, cleanup, stop, start, restart, and health check-now onto one fully durable `agent_jobs` queue.
- Add deeper agent reconciliation for side effects when event posts fail after Docker or Caddy changes.
- Add audit event storage and UI.
- Add durable rate-limit/backoff storage.
- Add dependency and image scanning to CI.
- Add release signing, static Linux assets, SBOMs, and GitHub artifact attestations.
- Add retention/cleanup policies for deployment logs, images, failed containers, webhooks, agent jobs, and deeper disk usage reporting.
- Re-enable remote self-hosted VPS agents only after prebuilt agent binaries, token rotation/revoke, systemd install/uninstall, and disposable VPS validation exist.
