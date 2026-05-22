# Current Capability and Gap Report

Date: 2026-05-22

This document describes the updated `0.1.0` package. Hostlet is a local-machine-only, single-owner homelab deployment beta. It should not be described as a multi-user production platform or remote VPS fleet manager.

## Current Capabilities

- First-run setup token, control-plane password, unlock gate, logout, and GitHub Device Flow login.
- Optional GitHub login allowlist with encrypted GitHub tokens.
- GitHub repository listing and private repository deploys through tokenized fetch URLs with log redaction.
- Local deployment agent connected over authenticated WebSocket/events.
- Dockerfile-based deploys and generated Dockerfiles for common Node apps.
- Configurable root directory, install/build/start commands, container port, health path, CPU limit, and memory limit.
- Encrypted app environment variables with key-only display and explicit replace/delete UI.
- Per-app deploy, rollback, delete, public URL publishing, and auto-redeploy controls.
- Per-app Cloudflare DNS publication under the configured base domain, with ownership tracking in `app_public_dns_records`.
- GitHub push webhook handling with signature verification, delivery dedupe, exact commit deploys, and per-app webhook status.
- Automatic GitHub webhook create/update when auto-redeploy is enabled and the GitHub token has hook permissions.
- Live deployment logs, stored deployment logs, and basic Docker resource stats for local apps.
- App deletion requests local agent cleanup of managed containers, images, Caddy routes, app data volume, public DNS, resource snapshots, and app database records.
- Production Dockerfiles, production Compose, CLI setup wizard, `hostlet doctor`, `hostlet backup`, and `hostlet restore`.
- API origin/CSRF checks, signed cookies, signed agent jobs, request body limits, and in-memory rate limits for setup/unlock/OAuth/webhook/agent endpoints.

## Explicit 0.1.0 Scope Limits

- Remote VPS agents are disabled. The UI, API, Postgres, Caddy, local agent, and deployed app containers run on the same host.
- Deploy and rollback jobs are delivered through the connected in-memory agent WebSocket. There is a per-app active deployment lock, but no durable queue, retry worker, cancellation, or job claim protocol.
- Delete-app cleanup has an `agent_jobs` row, but finalization is still performed by an API task and needs durable reconciliation before production use.
- Generated static Node deploys assume `dist` and run `npx serve` at container startup.
- Runtime presets beyond Dockerfile and Node are not first-class.
- Audit events, audit UI, RBAC, team support, active-session UI, and app-level permissions are not implemented.
- Rate limits are in-memory and reset when the API restarts.
- Dependency/image scanning, release signing, SBOMs, and provenance attestations are not enforced.
- Backups are local scripts only. Scheduled backup, off-host backup, and clean-machine restore validation remain operator work.
- Log, image, failed-container, webhook, agent-job, and resource-snapshot retention policies are not implemented.

## Updated 0.1.0 Packaging Work

- Keep the release version string at `0.1.0` and republish the artifacts/checksum from the current tree.
- Validate `hostlet init`, `hostlet up`, `hostlet doctor`, deploy, rollback, auto-redeploy, publish/private URL, delete, backup, and restore on a clean owner-controlled host.
- Document the exact release commit and checksum after repackaging.

## Later Reliability Track

- Move deploy, rollback, delete, cleanup, stop, start, and restart onto one durable `agent_jobs` queue.
- Add agent claim/retry/reconciliation and startup recovery for unfinished jobs.
- Add audit event storage and UI.
- Add durable rate-limit/backoff storage.
- Add dependency and image scanning to CI.
- Add release signing, static Linux assets, SBOMs, and GitHub artifact attestations.
- Add retention/cleanup policies and disk usage reporting.
- Re-enable remote VPS agents only after prebuilt agent binaries, token rotation/revoke, systemd install/uninstall, and disposable VPS validation exist.
