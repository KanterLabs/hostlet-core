# Hostlet 0.2.0 Plan

Date: 2026-05-24

This plan defines the next major Hostlet update after the local-machine-only `0.1.0` beta. The goal for `0.2.0` is to make Hostlet feel dependable after the first deploy: it should keep checking running apps, clearly show current health without forcing manual refreshes, and make Hostlet itself easy to update.

## Product Goals

- Recurring runtime health checks for deployed apps.
- Clear app health status in the dashboard, separate from deployment status.
- Better live data refresh across app list, app detail, settings, and operations pages.
- Update detection for Hostlet itself.
- A very simple update flow for owners, ideally one command from the CLI and one guided action from the UI.
- User manual refresh that matches the current product and removes stale setup guidance.

## Non-Goals For 0.2.0

- Multi-user/team support.
- Remote VPS agent fleet support.
- Fully automatic unattended updates by default.
- Kubernetes, Nomad, or multi-host scheduling.
- Paid SaaS update channels.

Hostlet should remain a single-owner homelab deployment tool in `0.2.0`, but it should be much safer and easier to maintain.

## Phase 1: Runtime Health Monitoring

### Data Model

Add migrations for:

- `app_health_snapshots`: latest health per app/current container.
- `app_health_events`: recent health history.
- optional health settings on `apps` or a new `app_health_settings` table.

Recommended `app_health_snapshots` fields:

- `app_id`
- `deployment_id`
- `container_name`
- `status`: `unknown`, `healthy`, `degraded`, `unhealthy`
- `checked_url`
- `http_status`
- `latency_ms`
- `failure_count`
- `success_count`
- `last_error`
- `last_checked_at`
- `last_healthy_at`
- `updated_at`

Recommended `app_health_events` fields:

- `app_id`
- `deployment_id`
- `container_name`
- `status`
- `checked_url`
- `http_status`
- `latency_ms`
- `error`
- `created_at`

Retention default:

- Keep latest snapshot forever while app exists.
- Keep health events for 7 days or the latest 500 events per app, whichever is easier to implement safely first.

### Agent Behavior

The agent already performs deploy-time health checks. In `0.2.0`, split that logic into reusable health probe code and add a recurring monitor loop.

Default behavior:

- Check current deployed apps every 60 seconds.
- Use the app's configured `container_port` and `health_path`.
- Use a 5 second request timeout.
- Treat HTTP `2xx` and `3xx` as passing unless a stricter app setting is added.
- Mark `degraded` after 1 failure.
- Mark `unhealthy` after 3 consecutive failures.
- Mark `healthy` after 1 successful check following failure.

The agent should report:

- app id
- deployment id
- container name
- checked URL
- status code
- latency
- error summary
- consecutive failure count

Container checks should include:

- container exists
- container is running
- mapped loopback port is still reachable
- HTTP health path responds

### API Behavior

Add endpoints:

- `GET /api/apps/:id/health`
- `GET /api/apps/:id/health/events`
- `POST /api/apps/:id/health/check-now`
- `GET /api/health/summary`

Extend existing app list/detail JSON with:

- latest runtime health
- last checked timestamp
- last healthy timestamp
- failure count

Important rule:

- Deployment status and runtime health are separate. A deployment can be `success` while runtime health is later `unhealthy`.

### UI Behavior

App list:

- Show a health pill next to deployment status.
- Add filters: `healthy`, `degraded`, `unhealthy`, `unknown`.
- Show last checked time in the app row.

App detail:

- Add a Runtime Health section above resource usage.
- Show current status, latency, status code, last checked, last healthy, and failure reason.
- Add a `Check now` button.
- Show recent health events in a compact timeline/table.

Dashboard:

- Add counts for healthy, degraded, unhealthy, and unknown apps.
- Surface unhealthy apps first.

## Phase 2: Better Live Data Refresh

The current UI often needs manual refreshes to see updated status. `0.2.0` should make the dashboard feel live without requiring a full real-time rewrite.

### Short-Term Implementation

Use focused polling first:

- App list: refresh every 10 seconds.
- App detail health: refresh every 5 seconds.
- Server status: refresh every 10 seconds.
- Update status: refresh every 30 minutes, plus manual check.
- Pause polling when the tab is hidden.
- Avoid resetting form state while the user is editing settings.

### Later Upgrade Path

After polling is stable, add one authenticated browser event stream or WebSocket for:

- deployment status
- runtime health changes
- agent online/offline changes
- update availability

Polling is acceptable for `0.2.0`; correctness matters more than fancy real-time infrastructure.

## Phase 3: Self-Healing Controls

Start conservative. Detection should ship before automatic repair.

### 0.2.0 Minimum

- Show unhealthy state clearly.
- Provide manual `Restart container` action.
- Provide manual `Redeploy latest` action.
- Provide manual `Rollback` action where a previous successful deployment exists.

### Optional 0.2.x Follow-Up

Per-app self-healing settings:

- auto-restart stopped container
- auto-restart after N failed checks
- rollback after restart fails

Required guardrails:

- cooldown between repair attempts
- daily max repair attempts per app
- audit event for each repair
- never delete failed containers before logs are captured
- keep self-healing off by default

## Phase 4: Hostlet Update Detection

Hostlet needs to detect when a newer release is available and explain the safest update path.

### Version Source

Use GitHub Releases as the default update source:

- repository: `ShaneKanterman04/Hostlet`
- channel: stable releases only by default
- latest version from release tags such as `v0.2.0`
- release assets for CLI binary and checksum

Add a release manifest asset in future releases:

```json
{
  "version": "0.2.0",
  "released_at": "2026-05-24T00:00:00Z",
  "minimum_supported_version": "0.1.0",
  "assets": {
    "hostlet-linux-x64": {
      "sha256": "..."
    }
  },
  "compose_migrations": true,
  "database_migrations": true,
  "notes_url": "https://github.com/ShaneKanterman04/Hostlet/releases/tag/v0.2.0"
}
```

If the manifest is missing, fall back to GitHub release metadata and the `.sha256` asset.

### CLI Commands

Add:

```bash
hostlet version
hostlet update check
hostlet update
hostlet update --dry-run
hostlet update --yes
hostlet update rollback
```

`hostlet update check` should:

- print current version
- print latest version
- show release notes URL
- say whether an update is available
- warn about unsupported upgrade paths

`hostlet update --dry-run` should:

- verify Docker and Compose access
- verify `.env` exists
- verify the working directory is a Hostlet install
- verify enough disk space for backup and new images
- verify release asset checksum is available
- show exactly what will be changed

`hostlet update` should:

1. Check for latest release.
2. Show release notes and version jump.
3. Create a pre-update backup.
4. Download the new CLI binary to a temporary path.
5. Verify SHA256.
6. Replace the installed CLI binary.
7. Pull or rebuild updated Hostlet service images as required.
8. Run database migrations through the API startup path.
9. Restart Hostlet services with Compose.
10. Run `hostlet doctor`.
11. Print success or rollback guidance.

### UI Update Panel

Add a Settings or System page panel:

- current Hostlet version
- latest available version
- last update check time
- release notes link
- `Check for updates` button
- guided update command

The first UI version can show the command to run:

```bash
hostlet update
```

Do not run privileged host updates directly from the web UI in `0.2.0`. The web UI should detect and guide; the CLI should perform the update.

### API Update Endpoint

Add endpoints:

- `GET /api/system/version`
- `POST /api/system/update-check`

The API should not need GitHub auth for public releases.

Cache update checks:

- automatic check every 24 hours
- manual check on demand
- store latest result in `settings` or a small `system_update_checks` table

Privacy and reliability:

- allow disabling update checks with env var, for example `HOSTLET_UPDATE_CHECKS=false`
- handle offline mode cleanly
- never block app deployment if update checking fails

## Phase 5: Safer Update Process

The update flow must be hard to break and easy to understand.

### Backup Before Update

`hostlet update` should create a backup automatically unless `--no-backup` is explicitly passed.

Backup should include:

- Postgres dump
- agent state volume
- Caddy route snippets
- current `.env` copy or `.env` checksum plus clear instruction, depending on secret handling decision
- current Hostlet version metadata

### Rollback

`hostlet update rollback` should restore the previous service version when possible.

Minimum rollback:

- restore previous CLI binary from backup
- restore previous Compose file/service image tags if versioned
- restart services
- point user to the pre-update backup

Database rollback is harder. For `0.2.0`, prefer forward-compatible migrations and clearly state when DB rollback is not automatic.

### Release Asset Improvements

Expand release packaging:

- versioned Compose files or image tags
- CLI checksum
- release manifest
- changelog/release notes
- static Linux x64 binary if practical

Later:

- arm64 Linux binary
- signed checksums
- SBOM
- GitHub artifact attestations

## Phase 6: Operations And Doctor Improvements

Extend `hostlet doctor` with:

- current Hostlet version
- latest version check result
- Docker socket access
- Compose project health
- API `/health`
- web service reachability
- agent heartbeat freshness
- Caddy route import sanity
- cloudflared running when tunnel mode is configured
- per-app runtime health summary
- disk space warning
- backup freshness warning

Add:

```bash
hostlet status
```

Recommended output:

- Hostlet version
- services up/down
- agent online/offline
- app health counts
- latest backup
- latest available update

## Phase 7: User Manual Refresh

The manual should be refreshed after the health and update flows are implemented, not before. The updated manual should be shorter and task-oriented.

Recommended structure:

- Install Hostlet
- Update Hostlet
- Create your first app
- Configure app settings
- Health checks
- Logs and troubleshooting
- Public URLs and Cloudflare
- Auto redeploy
- Backup and restore
- Doctor and status commands
- Known limits

Specific docs to update:

- root `README.md`
- `docs/README.md`
- `docs/ARCHITECTURE.md`
- `docs/FEATURE_GAPS.md`
- release notes for `0.2.0`
- validation checklist for `0.2.0`

Remove or clearly mark historical `0.1.0` plans so users do not confuse old planning docs with the current manual.

## Implementation Order

1. Add health data model and agent health event reporting.
2. Add API health endpoints and app response fields.
3. Add app list/detail health UI and polling refresh.
4. Add manual `Check now`.
5. Add update check logic in CLI.
6. Add update status API and UI panel.
7. Add `hostlet update --dry-run`.
8. Add guarded `hostlet update`.
9. Extend `hostlet doctor` and add `hostlet status`.
10. Refresh manual and release notes.

## Release Gates

Before tagging `v0.2.0`:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

Manual validation:

1. Install `0.1.0`.
2. Deploy a test app.
3. Upgrade to `0.2.0` with `hostlet update`.
4. Confirm app still serves.
5. Confirm runtime health appears without refreshing.
6. Break the app health path and confirm Hostlet marks it degraded/unhealthy.
7. Restore the health path and confirm recovery.
8. Run `hostlet doctor`.
9. Run backup and restore validation.

Track the full pre-tag checklist in [VALIDATION_0.2.0.md](VALIDATION_0.2.0.md).

## Open Decisions

- Should health checks use the direct container loopback URL, public Caddy route, or both?
- Should HTTP `3xx` count as healthy by default?
- Should update checks happen only in the CLI, or also from the API on a 24 hour timer?
- Should the update flow use prebuilt Docker images or local rebuilds from the updated repository?
- How much rollback should `hostlet update rollback` promise before signed/versioned service images exist?
