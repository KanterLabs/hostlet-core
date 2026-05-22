# Hostlet 0.1.0 Autonomous Implementation Plan

Date: 2026-05-21

Status update, 2026-05-22: this is a historical implementation plan. The updated `0.1.0` package is local-machine-only, with app settings/env editing, app delete cleanup, opt-in auto-redeploy, backup/restore commands, production Compose, and in-memory rate limits implemented. Current capabilities and remaining limits are summarized in [FEATURE_GAPS.md](FEATURE_GAPS.md) and [SHIP_PLAN_0.1.0.md](SHIP_PLAN_0.1.0.md).

This plan contains work Codex can implement independently in the repository and local environment. External account validation is called out separately in [Owner Input Plan](PLAN_OWNER_INPUT_0.1.0.md).

## Default Product Decisions

Use these defaults unless Shane explicitly overrides them:

- `0.1.0` scope: secure single-owner homelab beta.
- Auto-redeploy: opt-in per app and per branch. Default off.
- Deploy concurrency: reject a new deploy when the same app already has an active deploy. Do not queue for `0.1.0`.
- App delete: remove all Hostlet-managed containers, images, Caddy snippets, resource snapshots, and Hostlet-managed DNS for that app.
- Failed deployment containers: preserve initially for debugging, but expose cleanup.
- Backup target: local archive directory for `0.1.0`; external backup destinations can wait.
- Remote VPS support: disabled for the updated `0.1.0` package.
- Public exposure: optional per app. Default private.
- Production packaging: Docker image plus production Compose, not a full installer.

## Implementation Status

This plan has been implemented in the repository for the `0.1.0` autonomous scope:

- app delete now requests agent teardown for Hostlet-managed containers, images, routes, DNS, and resource snapshots
- manual and webhook deployments reject active same-app deployments
- stale active deployments are failed on API startup after 30 minutes
- auto-redeploy is opt-in per app and records webhook app events
- app detail UI can edit runtime settings and encrypted environment variables
- logout and settings diagnostics are available in the UI
- Cloudflare configuration status is exposed without leaking tokens
- local backup/restore scripts were added
- production Dockerfiles, production Compose, and CI were added

Remaining validation that depends on owner-controlled external systems is tracked in [Owner Input Plan](PLAN_OWNER_INPUT_0.1.0.md).

## Phase 1: Runtime Safety

### 1. App Teardown

Implement:

- API delete flow that gathers app runtime metadata before DB deletion.
- Agent job type `delete_app`.
- Container stop/remove for current and historical app containers.
- Image cleanup for Hostlet-built app images.
- Caddy route snippet removal.
- Cloudflare DNS deletion for Hostlet-managed app domain if public exposure is open.
- `app_resource_snapshots` cleanup.
- Clear user-facing delete failure messages.

Verify locally:

- Create app, deploy, open tunnel, delete app.
- Confirm no app containers remain.
- Confirm no Caddy route file remains.
- Confirm DB rows are removed.
- Confirm public link no longer resolves when Cloudflare credentials are available; otherwise verify API calls are gated and mocked/manual DNS path is documented.

### 2. Deploy Lock

Implement:

- Active deployment detection by app.
- Reject manual deploy when active deploy exists.
- Reject webhook-triggered deploy when active deploy exists.
- UI disabled state and clear message.
- Tests for active status selection.

Recommended active statuses:

- `queued`
- `running`
- `building`
- `starting`
- `health_checking`
- `routing`

Verify locally:

- Double-click deploy.
- Send deploy request twice with curl.
- Simulate webhook while deploy is active.

### 3. Stale Deployment Recovery

Implement:

- API startup recovery for active deployments older than a threshold.
- Failure summary explaining API/agent restart interrupted the deployment.
- Optional agent heartbeat/status event that can be extended later.

Recommended threshold:

- 30 minutes for `queued/running/building/starting/health_checking/routing`.

Verify locally:

- Insert active deployment row with old timestamp.
- Restart API.
- Confirm stale deployment is marked failed with clear message.

## Phase 2: Auto-Redeploy on Branch Push

### 1. Data Model

Implement migrations for:

- `apps.auto_deploy BOOLEAN NOT NULL DEFAULT false`
- webhook delivery metadata fields/table for app matching and deploy result

Recommended metadata table:

- `webhook_events` can be extended with `branch`, `commit_sha`, `matched_app_count`, `deployment_id`, `ignored_reason`.

### 2. API Behavior

Implement:

- app create/update support for `auto_deploy`
- webhook deploy only when `repo_full_name`, `branch`, and `auto_deploy=true` match
- exact pushed commit SHA passed to deploy job
- deploy lock applied before creating webhook deployment
- ignored webhook events recorded with reason

Verify locally:

- Unit/integration tests for branch matching.
- Simulated GitHub push payloads with valid HMAC.
- Confirm disabled app does not deploy.
- Confirm different branch does not deploy.

### 3. UI Behavior

Implement:

- Create app checkbox: **Auto deploy on push to this branch**
- App settings toggle for auto-deploy
- App detail summary showing auto-deploy state
- Last webhook delivery/result display
- Webhook setup instructions showing URL and secret name, not secret value

Verify locally:

- Create app with auto-deploy disabled.
- Enable from settings.
- Simulate webhook and see result in UI.

## Phase 3: App Configuration UX

### 1. App Settings Page

Implement:

- settings route or tab for each app
- edit domain, health path, root directory, install command, build command, start command, memory limit, CPU limit
- save/revert UX
- clear “requires redeploy” indication

### 2. Environment Variable Editor

Implement:

- list env var keys only
- add/update/delete env vars
- never display decrypted values
- explicit “replace value” flow
- server-side validation remains authoritative

Verify locally:

- Add env var, deploy app that reads it.
- Replace env var, redeploy, verify changed value.
- Delete env var, redeploy, verify missing value.

## Phase 4: Auth and Account UX

Implement:

- `/api/logout` route that expires session, unlock, OAuth state, and web-origin cookies.
- Logout button in navigation or settings.
- GitHub reconnect control in settings.
- GitHub status panel with token-valid state and reconnect action.

Verify locally:

- Logout redirects or blocks API access.
- Reconnect refreshes repo listing.

## Phase 5: Cloudflare Diagnostics

Implement:

- Cloudflare status endpoint.
- Validate token can access zone.
- Validate tunnel target is configured.
- Validate app hostname is Hostlet-managed before opening tunnel.
- UI status showing configured/missing/error.
- DNS propagation guidance after opening tunnel.

Verify locally:

- Missing config shows actionable message.
- Invalid token shows actionable message without exposing token.
- Managed hostname guard still prevents apex/portfolio record changes.

## Phase 6: Backup and Restore

Implement:

- `scripts/backup.sh`
- `scripts/restore.sh`
- docs for backup contents and restore validation

Recommended backup contents:

- Postgres dump
- `.env` template/checklist without printing secrets
- `/var/lib/hostlet` agent state if present

Verify locally:

- Backup current test stack.
- Restore into clean volumes.
- Confirm app metadata exists and encrypted values decrypt with original `ENCRYPTION_KEY`.

## Phase 7: Production Packaging

Implement:

- API production Dockerfile.
- Web production Dockerfile.
- `infra/docker-compose.prod.yml`.
- `.dockerignore`.
- production docs update.

Recommended production shape:

- API binary image built from Rust multi-stage Dockerfile.
- Web standalone Next image.
- Postgres with named volume.
- Local agent image or service using Docker socket.
- Caddy/cloudflared optional services.

Verify locally:

- Build all production images.
- Start prod Compose.
- Health check API and web.

## Phase 8: Remote Agent Install

Status for updated `0.1.0`: remote agent install is intentionally disabled. The historical tasks below are deferred until a later remote-agent release:

- remove fake `https://github.com/example/Hostlet.git` default
- make `HOSTLET_REPO_URL` required or configure real default
- generated install command includes a real repo URL or required repo URL configuration
- add reinstall/troubleshooting docs

Recommended default:

- Make `HOSTLET_REPO_URL` required until a canonical public repo URL is confirmed by Shane.

Verify locally:

- Script shellcheck-style review.
- Dry-run where feasible.
- If no VPS is available, mark full remote validation as owner-required.

## Phase 9: Release Automation

Implement:

- CI workflow for Rust fmt/test/clippy.
- Web typecheck.
- Compose config validation.
- Optional docs link check.
- Smoke-test checklist update.

Verify locally:

- Run the same commands before final handoff.

## Should-Fix Work Codex Can Also Do

These are not release blockers unless the scope changes:

- audit events
- rate limiting/request limits
- runtime log viewer
- server detail page and token rotation
- cleanup/retention UI
- better error messages

## Local Completion Criteria

Codex can call the autonomous track complete when:

- all code changes are implemented
- migrations apply cleanly
- existing data is preserved
- local smoke tests pass
- web and API are healthy
- docs match actual behavior
- owner-required validation list is reduced to external account checks only
