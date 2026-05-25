# Hostlet 0.3.0 Plan

Date: 2026-05-25

This plan defines the next major Hostlet update after `0.2.0`. The goal for `0.3.0` is to make local Hostlet operations durable, observable, and cleanly recoverable. Remote VPS agents should remain deferred until the local job system, audit trail, retention policies, and release supply chain are strong enough to support a multi-host model.

## Product Goals

- Durable operations for deploy, rollback, delete, restart, and health check-now.
- Clear recovery after API, agent, Docker, or host restarts.
- Owner-visible audit history for high-impact actions.
- Retention and cleanup policies for logs, images, containers, webhook records, health history, jobs, and resource snapshots.
- Stronger release artifacts with signing, SBOMs, and provenance.
- Better backup scheduling and restore validation.
- A firm readiness checklist for deciding whether remote agents can be reintroduced in a later release.

## Non-Goals For 0.3.0

- Multi-user/team support.
- Remote VPS fleet management.
- Automatic self-healing enabled by default.
- Kubernetes, Nomad, or external schedulers.
- Paid SaaS update channels.

`0.3.0` should keep Hostlet a single-owner local deployment tool, but it should behave more like an appliance: actions survive restarts, cleanup is predictable, and the owner can understand what happened.

## Phase 1: Durable Agent Job Queue

Move all agent-driven operations onto one durable `agent_jobs` queue:

- deploy
- rollback
- app delete cleanup
- restart current container
- health check-now
- stop/start current container if the UI exposes those actions
- Caddy route reconciliation
- Docker cleanup tasks

Recommended `agent_jobs` fields:

- `id`
- `server_id`
- `app_id`
- `deployment_id`
- `kind`
- `status`: `queued`, `claimed`, `running`, `succeeded`, `failed`, `cancelled`, `expired`
- `priority`
- `payload_json`
- `result_json`
- `attempt`
- `max_attempts`
- `claimed_by`
- `claimed_at`
- `lease_expires_at`
- `started_at`
- `finished_at`
- `last_error`
- `created_at`
- `updated_at`

Agent behavior:

- Claim jobs through an authenticated API endpoint instead of relying only on in-memory WebSocket delivery.
- Use short leases and renew while running long builds.
- Release or retry jobs whose lease expires.
- Make job handlers idempotent where practical.
- Report structured progress events and final results.
- Preserve logs for failed deploys and failed cleanup.

API behavior:

- Enqueue jobs inside the same transaction that creates the user-visible operation record.
- Expose job status in app detail and deployment detail responses.
- Reconcile unfinished jobs on API startup.
- Prevent conflicting jobs per app with a durable lock or conflict policy.

## Phase 2: Recovery And Reconciliation

Add startup reconciliation for both API and agent.

API startup should:

- Mark stale claimed/running jobs as retryable or failed based on age and attempt count.
- Rebuild current app/deployment status from database state.
- Detect apps whose latest deployment points at missing health or route metadata.
- Keep webhook dedupe records intact across restarts.

Agent startup should:

- Register heartbeat and capability metadata.
- Reconcile managed containers for current deployments.
- Verify Caddy route files match current deployment records.
- Report missing containers, stale route files, and orphaned managed containers.
- Resume queued work without requiring a browser action.

Owner-facing recovery:

- Add an Operations page or app-level Operations table.
- Show active, failed, and recently completed jobs.
- Provide retry for safe failed jobs.
- Provide cancel for queued jobs and best-effort cancel for running jobs where the handler supports it.

## Phase 3: Audit Events

Create an append-only audit event table for high-impact actions.

Events to capture:

- setup password set or changed
- unlock failures above threshold
- GitHub account connected or disconnected
- app created, updated, deleted
- deployment requested, succeeded, failed
- rollback requested
- public URL published or made private
- app environment variables changed by key name only
- restart/check-now requested
- backup, restore, update, and rollback commands started or completed
- update check result
- settings changed

Recommended fields:

- `id`
- `actor_type`: `owner`, `system`, `agent`, `webhook`, `cli`
- `actor_id`
- `event_type`
- `app_id`
- `deployment_id`
- `job_id`
- `ip_address`
- `user_agent`
- `metadata_json`
- `created_at`

UI behavior:

- Add a compact audit timeline on app detail for app-scoped events.
- Add a global audit view under Settings or Logs.
- Avoid storing secrets, raw tokens, or full environment values.

## Phase 4: Retention And Cleanup Policies

Add explicit retention settings with conservative defaults.

Recommended defaults:

- deployment logs: keep 30 days and latest 20 deployments per app
- health events: keep 7 days and latest 500 events per app
- resource snapshots: keep 7 days and latest 1,000 snapshots per app
- webhook events: keep 14 days
- completed agent jobs: keep 30 days
- failed agent jobs: keep 90 days
- failed deploy containers/images: keep latest 3 per app or 7 days
- old successful images: keep current plus previous successful deployment

Implementation notes:

- Run cleanup through durable `agent_jobs` where Docker state is affected.
- Keep dry-run support in `hostlet doctor` or a new `hostlet cleanup --dry-run`.
- Never delete the current deployment container/image.
- Never delete logs for an active or retryable job.
- Record cleanup actions in audit events.

## Phase 5: Backup Scheduling And Restore Confidence

Improve backups from manual scripts into an owner-visible operational feature.

Minimum:

- Add `hostlet backup --scheduled` or a documented systemd timer template.
- Record backup metadata in the database or a local metadata file.
- Show latest backup age in Settings, `hostlet status`, and `hostlet doctor`.
- Add restore preflight checks for Docker, Compose, disk space, and target directory cleanliness.
- Add a clean-machine restore validation checklist to release validation docs.

Optional:

- Off-host backup target hooks for S3-compatible storage or rsync.
- Backup encryption using an owner-provided passphrase.
- Backup pruning policy.

## Phase 6: Release Supply Chain Hardening

Strengthen release artifacts before Hostlet starts managing more hosts.

Add to release workflow:

- signed checksums
- SBOM for Rust binaries and container images
- GitHub artifact attestations
- static Linux x64 binary if practical
- Linux arm64 binary if the build runner supports it reliably
- versioned Compose files or immutable image tags
- release manifest schema validation

CLI behavior:

- Verify checksum before replacing binaries.
- Prefer signed checksum verification when release signatures are available.
- Print a clear warning when only unsigned checksums are available.
- Keep rollback state compatible across at least one minor release.

## Phase 7: Remote Agent Readiness Checklist

Do not re-enable remote VPS agents in `0.3.0` unless all readiness items are complete. The expected outcome for `0.3.0` is a checklist and any low-risk groundwork, not fleet management.

Required before a remote-agent release:

- prebuilt agent binaries
- agent install and uninstall commands
- systemd unit templates
- token rotation and revoke
- per-agent capability reporting
- per-agent version compatibility checks
- durable queue claim protocol proven locally
- disposable VPS validation
- firewall and ingress documentation
- clear trust-boundary documentation

## Implementation Order

1. Extend `agent_jobs` schema and add queue claim/lease endpoints.
2. Convert restart and health check-now to durable jobs first.
3. Convert delete cleanup to durable jobs and remove API-task finalization.
4. Convert deploy and rollback to durable jobs.
5. Add startup reconciliation for stale jobs and managed containers.
6. Add Operations UI and retry/cancel controls.
7. Add audit event storage and app/global audit views.
8. Add retention settings and cleanup dry-run.
9. Add Docker cleanup jobs with guardrails.
10. Add backup scheduling metadata and restore preflights.
11. Add release signing, SBOMs, and attestations.
12. Refresh docs, feature gaps, architecture, release notes, and validation checklist.

## Release Gates

Automated checks:

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

1. Start a deploy, restart the API, and confirm the job recovers or fails clearly.
2. Start a deploy, restart the agent, and confirm the job recovers or fails clearly.
3. Queue conflicting operations for one app and confirm conflict handling is deterministic.
4. Delete an app and confirm containers, images, Caddy routes, app data volume, public DNS, and database records are reconciled through durable jobs.
5. Run cleanup dry-run, then cleanup, and confirm the current deployment remains intact.
6. Confirm audit events are created for deploy, rollback, restart, environment update, public URL changes, backup, and update actions.
7. Restore from backup into a clean disposable environment.
8. Validate release signatures, SBOMs, checksums, manifest, and update rollback.

## Open Decisions

- Should deploy logs be tied primarily to deployments, jobs, or both?
- Should cancellation be best-effort only for running Docker builds, or should the agent actively terminate build processes?
- Should retention settings live globally only, or allow per-app overrides?
- Should cleanup run on a timer by default, or only through explicit owner action in `0.3.0`?
- Should audit metadata store request IP addresses in LAN-only mode by default?
- Which signing tool should be standard for releases?
