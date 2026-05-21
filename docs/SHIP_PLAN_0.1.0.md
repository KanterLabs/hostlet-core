# Hostlet 0.1.0 Ship Plan

Date: 2026-05-21

Original mode for this pass: read-only planning. The autonomous implementation track has since been implemented in-repo; external validation remains owner-dependent.

This plan is split into two execution tracks:

- [Autonomous implementation plan](PLAN_AUTONOMOUS_0.1.0.md): work Codex can implement and verify locally without new external access.
- [Owner input plan](PLAN_OWNER_INPUT_0.1.0.md): external accounts, credentials, test resources, and release decisions needed from Shane.

## Target

Ship `0.1.0` as a secure single-owner homelab beta:

- one operator installs Hostlet on a trusted machine
- GitHub login works
- apps deploy from GitHub to the local machine
- app logs, rollback, resource stats, and optional Cloudflare Tunnel exposure work
- deleting and managing apps does not leave unsafe public routes or unmanaged containers behind

Do not position `0.1.0` as a multi-user production platform yet.

## Current Baseline

Already working or mostly working:

- first-run control-plane password and unlock flow
- GitHub OAuth login and repo listing
- local agent builds and runs Docker containers
- deploy from a GitHub repo and branch
- generated Dockerfile support for common Node apps
- deployment logs and live log stream
- health checks before routing
- rollback to previous successful deployment
- per-app Cloudflare Tunnel DNS open/close
- local resource stats reported by the agent
- GitHub push webhooks can trigger deploys for matching repo/branch pairs
- security hardening pass covering CORS, CSRF, signed sessions, signed OAuth state, signed agent jobs, log caps, and no Docker socket in the API

Major constraints still visible in code:

- app delete currently removes database rows only, not runtime resources
- app settings/env editing is API-only or missing
- settings page is a placeholder
- remote server UI and lifecycle are minimal
- direct in-memory agent job dispatch has no durable queue or per-app lock
- auto-redeploy is implicit for every matching webhook and has no app-level enable/disable, status, or setup guidance
- Compose is still development-oriented
- no backup/restore tooling
- no audit log or rate limiting

## Plan Review Notes

Pass 1 findings:

- Auto-redeploy should not be a vague future enhancement. The backend already deploys on GitHub push when repo and branch match, so `0.1.0` needs to make that behavior explicit, configurable, and testable.
- Deploy locking must apply to manual deploys and webhook-triggered deploys together, otherwise branch-push deploys can race manual deploys.
- The release smoke test needs a real branch-push check, not just a generic webhook trigger.

Pass 2 findings:

- Cleanup and settings work remain the biggest ship blockers because they affect trust during normal use.
- Cloudflare diagnostics should mention DNS propagation because a valid Tunnel record can still fail on stale client DNS.
- Production packaging and backup/restore are operational gates. They can be simple, but they need to exist before a tagged release.

## Release Definition

`0.1.0` is ready when these checks pass on a fresh machine:

1. Follow the README from zero to running Hostlet.
2. Set first-run password, unlock, and sign in with GitHub.
3. Create an app from a GitHub repo.
4. Deploy successfully to `This machine`.
5. View deployment logs and current resource usage.
6. Open the public tunnel, verify the URL externally, close the tunnel, verify DNS/route is removed.
7. Enable auto-redeploy for the selected branch, push a commit to that branch, and verify a new deployment starts for that commit.
8. Redeploy manually and rollback successfully.
9. Delete the app and verify containers, Caddy route, public DNS, resource snapshots, and app DB records are cleaned up.
10. Restart the Hostlet stack and verify existing deployed apps still serve.
11. Run backup and restore once against the test install.
12. Run the release checks: Rust fmt/test/clippy, web typecheck, Compose config, and a smoke test.

## Must Fix Before 0.1.0

### 1. App Delete Must Clean Runtime State

Current evidence:

- `apps/api/src/web.rs::delete_app` only deletes the app row.
- `apps/agent/src/main.rs` handles `deploy` and `rollback`, but not app teardown.
- Caddy snippets and Docker containers can remain after DB deletion.

Required work:

- Add an agent job type for app deletion.
- Stop and remove the current app container.
- Optionally remove old Hostlet containers/images for that app.
- Remove the Caddy route snippet for the app route key/domain.
- Delete the app's Cloudflare DNS record if `public_exposure=true`.
- Delete `app_resource_snapshots` rows for app containers.
- Make API app deletion transactional around the DB portion and report teardown failure clearly.

Acceptance:

- After deleting an app, `docker ps -a` has no containers for that app unless explicitly preserved.
- `/var/lib/hostlet/caddy` has no route file for the app.
- Cloudflare has no Hostlet-managed DNS record for the app hostname.
- The UI no longer lists the app.
- Recreating an app with the same name/domain does not collide with stale state.

### 2. App Settings and Environment Variable UI

Current evidence:

- `apps/web/app/settings/page.tsx` is placeholder text.
- `apps/api/src/web.rs::update_app` updates only domain, health path, env vars, and public exposure.
- Create app supports root directory, install/build/start commands, resource limits, but update does not.
- Create app currently sends `env: []`; there is no UI to create or edit env vars.

Required work:

- Add an app settings page or tabs on the app detail page.
- Add editable fields for domain, health path, root directory, install command, build command, start command, memory limit, CPU limit, and public exposure.
- Extend `UpdateApp` and SQL updates for all app settings.
- Add env var management with masked existing values and explicit replace/delete behavior.
- Show when changes require redeploy to take effect.
- Validate inputs client-side and server-side.

Acceptance:

- User can add, edit, and delete env vars without touching the database manually.
- User can change health path/build/runtime settings and redeploy.
- Existing env var values are never displayed in plaintext.
- Failed validation shows actionable UI errors.

### 3. Logout and GitHub Reconnect

Current evidence:

- There is no logout route in `apps/api/src/main.rs`.
- Web navigation has no logout control.
- GitHub reconnect exists indirectly through `GitHubStatus`, but account/session flows are not explicit.

Required work:

- Add `/api/logout` to expire session and unlock cookies.
- Add a logout button to the UI.
- Add a GitHub reconnect button/status in settings.
- Make reconnect replace the current stored token for the same GitHub account.

Acceptance:

- Logout clears access and returns user to login/unlock flow.
- Reconnect updates the GitHub token and repo listing works afterward.

### 4. Backup and Restore

Current evidence:

- No backup scripts exist.
- Docs warn about backups but do not provide runnable commands.

Required work:

- Add `scripts/backup.sh` for Postgres and relevant Hostlet state.
- Add `scripts/restore.sh`.
- Include `ENCRYPTION_KEY` handling guidance without printing secret values.
- Add docs for backup location, restore process, and validation.

Acceptance:

- Backup from a test install restores into a fresh install.
- Restored install can list apps and decrypt GitHub/env data with the original `ENCRYPTION_KEY`.

### 5. Production-Oriented Packaging

Current evidence:

- `infra/docker-compose.yml` runs `cargo run` and `pnpm dev` from bind-mounted source.
- There are no production Dockerfiles for API/web.

Required work:

- Add production Dockerfiles for `hostlet-api` and `hostlet-web`.
- Add `infra/docker-compose.prod.yml`.
- Keep development Compose separate.
- Avoid bind-mounting the full source tree in production.
- Use non-root users where feasible.
- Document upgrade and migration flow.

Acceptance:

- A clean checkout can build release images.
- Production Compose starts API/web/Postgres/local-agent/Caddy/cloudflared without dev servers.
- README remains simple and points production users to the production guide.

### 6. Remote Agent Install Must Be Real

Current evidence:

- `scripts/install-agent.sh` defaults `HOSTLET_REPO_URL` to `https://github.com/example/Hostlet.git`.
- The generated install command from `apps/api/src/web.rs::create_server` does not include a real repo URL.
- Machines UI has no server detail/reinstall/token rotation flow.

Required work:

- Set a correct default repository URL or make `HOSTLET_REPO_URL` required.
- Include `HOSTLET_REPO_URL` in generated install instructions.
- Show install prerequisites and supported OS.
- Add reinstall instructions after the one-time install token is consumed.

Acceptance:

- A new VPS can run the generated command and connect without editing the script manually.
- Failed install has clear troubleshooting steps.

### 7. Prevent Concurrent Deploys Per App

Current evidence:

- `apps/api/src/deploy.rs::create_and_send_deploy` inserts and sends a deployment immediately.
- There is no per-app lock or state check before starting another deploy.

Required work:

- Reject or queue deploy requests when the same app has an active deployment.
- Define active statuses: queued, running, building, starting, health_checking, routing.
- Add UI disabled state and message.
- Ensure webhook deploys obey the same rule.

Acceptance:

- Double-clicking deploy cannot create two simultaneous deploys for one app.
- Push webhook during manual deploy does not race the manual deploy.

### 8. Auto-Redeploy on Branch Push

Current evidence:

- `apps/api/src/github.rs::webhook` already accepts GitHub `push` events.
- The webhook handler extracts `repository.full_name`, `ref`, and `after`.
- It currently deploys every app matching `repo_full_name` and `branch`.
- There is no `apps.auto_deploy` field, UI toggle, webhook setup status, or last-delivery status.

Required work:

- Add `apps.auto_deploy` with an explicit default.
- Recommended default: enabled for new apps only when the user checks **Auto deploy on push to this branch** during app creation.
- Add the same toggle to app settings.
- Update the webhook query to deploy only matching apps with `auto_deploy=true`.
- Store webhook delivery metadata: delivery ID, event, repo, branch, commit SHA, matched app count, deployment ID if started, ignored reason if not started.
- Show webhook URL and secret instructions for each app/repo.
- Show last push delivery and last auto-deploy result in the app UI.
- Ensure auto-deploy uses the exact pushed commit SHA, not branch `HEAD`.
- Ensure deploy locking applies before creating a webhook deployment.

Acceptance:

- User can enable auto-redeploy for one app and one branch.
- Pushing to that branch starts a deployment for the pushed commit.
- Pushing to a different branch does not deploy.
- Pushing to the same repo/branch for an app with auto-deploy disabled does not deploy.
- If a deploy is already active, the webhook is recorded and either skipped with a clear reason or queued according to the chosen deploy-lock behavior.
- The app UI shows the last webhook delivery and auto-deploy result.

### 9. Recover Stale In-Flight Deployments

Current evidence:

- Agent job dispatch is in-memory over WebSocket.
- API restart during a deployment can leave deployment state stale.

Required work:

- On API startup, mark old active deployments as failed or unknown with a clear message.
- On agent reconnect, report current active job/container status if possible.
- Add timestamps to detect stalled deployments.

Acceptance:

- Restarting API mid-deploy does not leave a permanent "running" deployment in the UI.

### 10. Cloudflare and Tunnel Diagnostics

Current evidence:

- Tunnel open/close exists, but config status and DNS diagnostics are not surfaced.
- User had to debug DNS from terminal.

Required work:

- Add API endpoint or settings status for Cloudflare config completeness.
- Validate token/zone/target without changing DNS.
- Show tunnel open/closed status and last DNS action.
- Add DNS propagation hints in UI after opening a tunnel.

Acceptance:

- User can tell whether Cloudflare is configured before clicking **Open tunnel**.
- DNS/API errors are shown as actionable messages.

### 11. Release Checks and CI

Current evidence:

- Checks are run manually.
- No CI workflow is present.

Required work:

- Add CI for:
  - `cargo fmt --all --check`
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `pnpm --dir apps/web lint`
  - `docker compose -f infra/docker-compose.yml config`
- Add a smoke-test checklist for local release candidates.

Acceptance:

- CI is green before tagging `v0.1.0`.

## Should Fix Before 0.1.0 If Time Allows

### 1. Audit Events

Add `audit_events` for:

- login/logout/unlock/setup
- app create/update/delete
- deploy/rollback
- tunnel open/close
- env var changes
- server create/register/revoke

Acceptance: app detail and settings can show recent audit events.

### 2. Rate Limiting and Request Limits

Add limits for:

- unlock attempts
- OAuth start/callback
- deploy/rollback
- webhook payload size
- agent event payload size

Acceptance: repeated bad unlock attempts slow down or fail safely.

### 3. Runtime Logs

Deployment logs exist, but post-deploy runtime logs are not first-class.

Add:

- app runtime log endpoint/job
- UI view for current container logs
- tail/refresh controls

Acceptance: user can inspect a running app without SSH.

### 4. Server Detail and Token Rotation

Add:

- machine detail page
- agent version/status/last seen
- Docker/Caddy status reported by agent
- rotate/revoke agent token
- uninstall/reinstall instructions

Acceptance: user can recover a broken remote agent without DB edits.

### 5. Cleanup and Retention Policy

Add:

- configurable retention for deployments/logs/images
- manual cleanup action
- disk usage status

Acceptance: old failed deploys do not accumulate forever without visibility.

### 6. Better Error Surfaces

Improve UI errors for:

- GitHub OAuth callback mismatch
- missing OAuth env vars
- GitHub token expired
- agent offline
- Docker build failure
- health check failure
- DNS propagation/cache issues

Acceptance: common failure messages suggest the next action.

## Can Wait Until After 0.1.0

- RBAC/team accounts
- GitHub App integration
- automatic GitHub webhook creation
- container vulnerability scanner UI
- registry push/pull
- per-app egress policies
- multi-machine scheduling
- blue/green traffic splitting
- custom domains not under `HOSTLET_DOMAIN_PREFIX`
- billing/quotas
- notification integrations

## Suggested Implementation Order

### Phase 1: Safety and Cleanup

1. App teardown agent job and delete flow.
2. Per-app deploy lock.
3. Auto-redeploy controls for branch pushes.
4. Stale deployment recovery.
5. Basic Cloudflare diagnostics.

Reason: these prevent the most confusing or unsafe states during normal use.

### Phase 2: Configuration UX

1. App settings page.
2. Env var editor.
3. Webhook setup/status surfaces in app settings.
4. Logout and GitHub reconnect.
5. Better settings/status page.

Reason: users should not need API calls or DB edits after app creation.

### Phase 3: Release Operations

1. Backup/restore scripts.
2. Production Dockerfiles and production Compose.
3. Remote agent install fix.
4. CI workflow and smoke checklist.

Reason: these make installation and upgrades repeatable.

### Phase 4: Quality and Observability

1. Audit events.
2. Rate limiting.
3. Runtime logs.
4. Cleanup/retention policy.
5. Server detail and token rotation.

Reason: these make the beta trustworthy and easier to debug.

## 0.1.0 Smoke Test

Run this before tagging:

1. Start from a clean Docker volume set.
2. Follow `README.md` setup.
3. Set password with `HOSTLET_SETUP_TOKEN`.
4. Connect GitHub.
5. Create a Node app without Dockerfile and deploy.
6. Create a Dockerfile app and deploy.
7. Confirm logs stream while deploying.
8. Confirm resource stats show after deployment.
9. Open tunnel and verify external URL.
10. Close tunnel and verify DNS removal.
11. Enable auto-redeploy for the app's branch.
12. Push a commit to that branch and verify Hostlet deploys that exact commit SHA.
13. Push a commit to a different branch and verify Hostlet does not deploy.
14. Roll back to previous success.
15. Delete app and verify cleanup.
16. Restart stack and verify remaining app still serves.
17. Run backup, restore into clean stack, and verify app metadata.
18. Run CI checks locally.

## Release Gate

Do not tag `v0.1.0` until all **Must Fix** items are complete or explicitly moved out of scope with a documented reason.
