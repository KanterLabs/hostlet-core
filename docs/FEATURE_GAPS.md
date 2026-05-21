# Missing Feature Report

Date: 2026-05-21

This report is based on a codebase audit of `apps/api`, `apps/agent`, `apps/web`, `infra`, migrations, scripts, and existing docs.

## Executive Summary

Hostlet has a working MVP for a single-owner homelab deployment panel:

- first-run password and unlock gate
- GitHub OAuth and repository listing
- local deployment agent
- app creation and deployment
- Dockerfile and generated Node builds
- health checks
- live deployment logs
- rollback routing
- optional per-app Cloudflare Tunnel DNS exposure
- basic resource stats for local apps

The largest remaining gaps after the autonomous `0.1.0` implementation pass are remote-agent maturity, audit/rate-limit controls, broader build support, and release smoke testing on owner-controlled external accounts.

Implemented since this audit:

- app teardown removes Hostlet-managed runtime state
- app settings and env var editing are available in the app detail UI
- auto-redeploy is explicit and opt-in
- backup/restore scripts exist
- production Dockerfiles and production Compose exist
- logout and GitHub/Cloudflare status panels exist

## Priority 0: Required Before Trusting Production Use

### Backup and Restore

Status: basic local backup/restore scripts now exist. Remaining gaps:

- scheduled backup
- off-host backup destination
- restore smoke test on a clean owner-controlled machine

Why it matters: losing Postgres or `ENCRYPTION_KEY` can permanently lose app configuration, encrypted env vars, GitHub tokens, deployment history, and rollback metadata.

Recommended work:

- add `scripts/backup.sh`
- add `scripts/restore.sh`
- document backup storage and restore validation
- add a Settings status check for whether backup is configured

### Production Packaging

Status: production Dockerfiles and `infra/docker-compose.prod.yml` now exist. Remaining gaps:

- pinned release images
- systemd unit or install script for the full Hostlet control plane
- migration procedure for upgrades

Recommended work:

- create `infra/docker-compose.prod.yml`
- build static release images for `hostlet-api` and `hostlet-web`
- document upgrade steps and migration safety

### Settings UI

Status: app settings, env vars, logout, GitHub status, and Cloudflare status now exist. Remaining gaps:

- one-click GitHub reconnect button from settings
- webhook setup instructions per app/repo
- change control-plane password

Current state: `apps/web/app/settings/page.tsx` is a placeholder and some app updates exist only through API.

Recommended work:

- add app settings tab/page
- add env var editor with masked existing values and explicit replace flow
- add global settings checks for GitHub, Cloudflare, secrets, and public URLs

### Audit Logging

Missing:

- audit table for user actions
- audit UI
- records for login, unlock, app create/update/delete, deploy, rollback, tunnel open/close, server registration, env changes

Recommended work:

- add `audit_events` table
- write audit events from API handlers
- display audit trail per app and globally

## Priority 1: Product Completeness

### App Lifecycle Management

Missing:

- stop/start/restart app
- delete app should remove containers, images, Caddy route, DNS record, resource snapshots
- cleanup old failed/superseded containers
- retention policy for deployments/logs/images
- promote/rollback UI clarity

Current state: deleting an app removes database rows but does not remove running containers. Failed containers are intentionally preserved but there is no cleanup workflow.

Recommended work:

- add agent jobs: `stop`, `start`, `restart`, `delete_app`, `cleanup_deployment`
- add cleanup screen and retention settings
- delete DNS/Caddy route on app delete when safe

### Auto Redeploy Management

Missing:

- UI to enable/disable auto deploy per app
- webhook creation automation
- visible webhook status and last delivery
- branch protection/deploy policy

Current state: GitHub push webhooks trigger deploys when manually configured and matching repo/branch.

Recommended work:

- add `apps.auto_deploy` boolean
- add webhook setup instructions per repo
- optionally create webhooks through GitHub API when permissions allow

### Remote Server Maturity

Missing:

- server delete/revoke
- rotate agent token
- reconnect instructions after install token is consumed
- remote resource stats
- remote Caddy status checks
- remote firewall/domain readiness checks
- server labels/regions

Current state: remote server creation, install commands, and agent registration are disabled for 0.1.0. Hostlet deploys to the same machine that runs the control plane.

Recommended work:

- add server detail page
- show agent version, OS, Docker version, Caddy status
- add token rotation and uninstall instructions

### Deployment Queue and Concurrency

Missing:

- durable job queue
- per-app deployment lock
- cancellation
- retry policy
- deploy timeout configuration
- handling API restart while jobs are in-flight

Current state: jobs are sent directly to connected agents over in-memory WebSocket channels. If API restarts during a job, the deployment record can become stale.

Recommended work:

- add job table/state machine
- agent claims jobs or reconnects with active job status
- prevent concurrent deploys for one app

### Build Support

Missing:

- Python, Go, Rust, static HTML, Docker Compose, monorepo workspace presets beyond Node/Dockerfile
- private submodules
- custom build args
- build cache controls
- registry push/pull
- image provenance/SBOM

Current state: Dockerfile repos work; no-Dockerfile Node apps are supported.

Recommended work:

- add runtime presets
- add custom build args and Docker target selection
- add optional registry support

## Priority 2: Security and Reliability Improvements

### Access Control

Missing:

- user roles
- team/org support
- app-level permissions
- session management UI
- logout button

Current state: effectively single-owner with optional GitHub login allowlist.

Recommended work:

- add owner/admin/viewer roles
- add logout and active sessions
- keep single-owner mode as default

### Rate Limiting and Abuse Protection

Missing:

- login/unlock rate limiting
- OAuth start/callback rate limiting
- deploy rate limiting
- webhook request size limit documented/enforced at ingress

Recommended work:

- add API rate limiting middleware
- add request body limits
- add lockout/backoff for password unlock attempts

### Supply Chain and Image Security

Missing:

- dependency audit in CI
- container image scanning
- generated image pinning by digest
- vulnerability report surface in UI

Recommended work:

- add CI for `cargo audit`, `pnpm audit`, and image scanner
- pin base images or track updates explicitly

### Network Policy

Missing:

- per-app egress controls
- app-to-app isolation policy
- configurable Docker networks
- option to block public exposure by policy

Recommended work:

- create per-app Docker networks
- add default-deny egress mode for advanced users

## Priority 3: UX and Observability

### Setup Wizard

Missing:

- guided setup for GitHub OAuth
- setup-token entry flow
- Cloudflare token/zone/tunnel validation
- DNS propagation diagnostics
- copyable webhook setup steps

Current state: setup depends mostly on docs and environment variables.

### Better Status Pages

Missing:

- control-plane health dashboard
- agent health details
- Cloudflare tunnel state
- Caddy route state
- Postgres migration version
- disk usage and Docker cleanup warnings

### Notifications

Missing:

- deployment success/failure notifications
- webhook failure alerts
- agent offline alerts
- email/Slack/Discord hooks

### Logs

Missing:

- search/filter
- download logs
- retention settings
- runtime log streaming after deploy

Current state: deployment logs are available, but app runtime logs after deployment are not first-class.

## Documentation Gaps Fixed In This Pass

Added or refreshed:

- root setup-first `README.md`
- complete `docs/README.md`
- architecture documentation
- security documentation
- this missing-feature report

Remaining docs to add later:

- production install guide once production packaging exists
- remote-agent hardening guide
- backup/restore runbook after scripts exist
- troubleshooting matrix for OAuth, DNS, Cloudflare Tunnel, Docker, and health checks

## Suggested Roadmap

1. Production packaging and backup/restore.
2. App settings/env editor and app lifecycle cleanup.
3. Audit events and rate limiting.
4. Auto-deploy controls and webhook setup automation.
5. Remote server detail page and token rotation.
6. Durable deployment queue and cancellation.
7. Broader build/runtime support.
