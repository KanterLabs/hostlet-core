# Hostlet 0.4.0 Hybrid Cloud Plan

## Goal

Ship Hostlet 0.4.0 as the first hybrid cloud release.

Hostlet remains open-source and fully self-hostable. Users can also connect a Hostlet Cloud account and deploy apps onto paid managed compute at `hostlet.cloud`.

Do not push implementation commits or tag `v0.4.0` until the full self-hosted path and local cloud simulation work.

## Architecture

For the MVP, `hostlet.cloud` points to one cloud VM. That VM runs:

- hosted Hostlet web panel
- Hostlet API
- Postgres
- Caddy/router
- managed Hostlet agent
- customer app containers

Managed app URLs use `*.apps.hostlet.cloud`.

Self-hosted panels are untrusted clients when they use Hostlet Cloud. They can authenticate to `hostlet.cloud` and request cloud deploys, but they never receive managed worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, direct database access, or raw job-queue access.

## Phase 1: Release Baseline And Modes

- Target version `0.4.0` for API, agent, and CLI.
- Add `HOSTLET_MODE=self_hosted|cloud`.
- Keep `self_hosted` as the default mode.
- Preserve existing local setup, unlock, GitHub Device Flow, local agent, and app deploy behavior.
- Add `docs/RELEASE_0.4.0.md` and `docs/VALIDATION_0.4.0.md` during implementation.

Acceptance: current self-hosted app create, deploy, logs, rollback, restart, publish/private, and delete flows still work.

## Phase 2: Single-VM Cloud Runtime

- Add cloud runtime config for the existing Compose shape.
- Use:
  - `PUBLIC_WEB_URL=https://hostlet.cloud`
  - `PUBLIC_API_URL=https://hostlet.cloud`
  - `PUBLIC_WEBHOOK_URL=https://hostlet.cloud`
  - `HOSTLET_BASE_DOMAIN=apps.hostlet.cloud`
- Run web, API, Postgres, Caddy, managed agent, and customer containers on the same VM.
- Do not add Cloud Build, Artifact Registry, Firestore, Kubernetes, or a worker fleet in 0.4.0.

Acceptance: local compose can run in normal self-hosted mode and dev cloud mode.

## Phase 3: Cloud User Tables

Add cloud-specific tables instead of treating local owner rows as cloud authority:

- `cloud_users`
- `cloud_sessions`
- `cloud_github_installations`
- `cloud_stripe_customers`
- `cloud_subscriptions`
- `cloud_plan_entitlements`
- `cloud_usage_buckets`

Keep the existing `users` table for self-hosted/local ownership behavior.

Acceptance: cloud users can exist alongside local users, and a local `users` row never implies Hostlet Cloud access.

## Phase 4: Hosted Cloud Panel And Account Flow

- Host the Next.js panel on `hostlet.cloud`.
- Add GitHub-first cloud login.
- Add GitHub App install flow for cloud repo access.
- Add Stripe checkout and customer portal.
- Hosted panel reads cloud user/session/billing state directly from the cloud API.

Acceptance: a user can sign into the hosted panel, install the GitHub App, and reach a billing-gated deploy screen.

## Phase 5: Self-Hosted Panel Cloud Link

- Add “Connect Hostlet Cloud” to self-hosted panels.
- Store only a scoped cloud session token locally.
- Show Hostlet Cloud as an optional deploy target beside “This machine.”
- Route cloud actions from local panels to `https://hostlet.cloud/api/...`.

Acceptance: local deploys work without cloud login; cloud deploys require a valid cloud session.

## Phase 6: Billing, GitHub App, And Entitlement Gates

- Keep Device Flow for self-hosted mode.
- Use GitHub App credentials for Hostlet Cloud.
- Gate cloud app creation, deploy, redeploy, restart, and rollback on server-side subscription and quota checks.
- Stripe state is authoritative only on `hostlet.cloud`.
- Cloud MVP limits:
  - single public web service
  - Dockerfile or generated Node app
  - 512 MiB RAM cap
  - 0.5 CPU cap
  - one exposed HTTP port
  - no cloud Compose
  - no custom domains
  - no persistent disks
  - no teams
  - no user-selected workers

Acceptance: unpaid users and linked self-hosted panels cannot bypass billing or create raw managed jobs.

## Phase 7: Cloud App Lifecycle On Same VM

- In cloud mode, cloud apps are owned by `cloud_users`.
- The cloud API assigns cloud apps to the local managed agent on the same VM.
- Users never choose the cloud server.
- Generate domains as `<app-slug>-<short-id>.apps.hostlet.cloud`.
- Reuse existing deploy, logs, health, rollback, restart, and delete job flow.

Acceptance: a paid cloud user can deploy a supported repo to the same VM and get a working `*.apps.hostlet.cloud` URL.

## Phase 8: Security And Abuse Controls

- Keep Cloudflare credentials only in the cloud VM environment.
- Keep Stripe secrets only in the cloud VM environment.
- Keep GitHub App private key only in the cloud VM environment.
- Keep managed agent tokens only in the cloud VM environment.
- Reject or ignore client-supplied managed server IDs.
- Add audit events for cloud login, checkout, app creation, deploy, rollback, restart, worker assignment, and suspension.
- Add clear failure states for unpaid account, quota exceeded, unsupported repo shape, no worker capacity, and missing GitHub installation.

Acceptance: unpaid users, revoked sessions, self-hosted panels, and forged worker IDs cannot reach managed compute.

## Phase 9: Local Validation Before Push

Run automated checks:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

Validate self-hosted mode:

- setup/unlock
- GitHub Device Flow
- local app create/deploy/logs/rollback/restart/publish/private/delete
- no Hostlet Cloud account required

Validate cloud mode locally:

- hosted panel in cloud mode
- cloud user/account flow
- mocked/test Stripe entitlement through the production entitlement code path
- GitHub App repo access path
- cloud app deploys to the same machine’s managed agent
- generated `*.apps.hostlet.cloud` equivalent routes through Caddy
- logs, health, rollback, restart, and usage work

Acceptance: no push until this phase passes.

## Phase 10: Push, CI, Tag, Release

- Commit only after local validation is green.
- Push implementation branch.
- Let CI pass.
- Tag `v0.4.0` only after CI passes.
- Push the tag to trigger the existing release workflow.
- Confirm release artifacts:
  - `hostlet-linux-x64`
  - `hostlet-linux-x64.sha256`
  - `hostlet-linux-x64.spdx.json`
  - `hostlet-release.json` with version `0.4.0`

## Assumptions

- `hostlet.cloud` is the cloud service domain.
- `hostlet.cloud` points to one VM for MVP.
- Postgres on that VM stores cloud account, billing, app, deploy, and usage data.
- Self-hosted installs have their own separate local Postgres.
- Open-source Hostlet remains fully usable without a cloud account.
- Hostlet Cloud is optional managed compute for hosted and self-hosted panels.
- Multi-worker scheduling, Cloud Build, Artifact Registry, Firestore, Kubernetes, custom domains, persistent disks, cloud Compose, and teams are deferred.
