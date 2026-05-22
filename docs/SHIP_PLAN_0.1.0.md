# Hostlet 0.1.0 Repackage Plan

Date: 2026-05-22

This plan keeps the public version at `0.1.0`. The goal is to republish the existing `v0.1.0` package from the updated tree with docs and artifacts that match the current local-only beta.

## Positioning

Ship `0.1.0` as a single-owner homelab beta:

- one trusted operator
- one local Hostlet machine
- GitHub Device Flow login
- GitHub app deploys to Docker on the local machine
- live deploy logs, resource stats, rollback, app settings, env vars, app delete cleanup, optional Cloudflare public URLs, and opt-in auto-redeploy

Do not position `0.1.0` as production hardened, multi-user, or remote VPS ready.

## Current Included Capabilities

- CLI wizard: `hostlet init`, `up`, `down`, `doctor`, `logs`, `backup`, and `restore`.
- Production Compose and production Dockerfiles for API, web, and agent.
- First-run setup token, password gate, logout, GitHub login, and GitHub reconnect.
- Local agent deploys Dockerfile repos and generated Node apps.
- App create/detail/settings UI with encrypted env-var management.
- Per-app public URL publish/private controls with Cloudflare DNS ownership tracking.
- Opt-in auto-redeploy per app; Hostlet creates or updates the GitHub webhook when permissions allow.
- Per-app active deployment lock.
- App delete cleanup through the local agent.
- Backup/restore scripts for Postgres and local agent state, with `.env` intentionally managed separately.
- In-memory rate limiting on setup, unlock, OAuth, webhook, and agent endpoints.

## Release Gates For Updated 0.1.0

Run these from the repository root before republishing:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

The Compose checks require a populated `.env` or equivalent exported variables.

## Clean-Host Smoke Test

On an owner-controlled machine:

1. Run `hostlet init`.
2. Start LAN mode with `hostlet up`; run `hostlet doctor`.
3. Set the first-run password, unlock, and connect GitHub.
4. Create an app targeting **This machine**.
5. Deploy successfully, inspect logs, and verify resource stats.
6. Edit an app setting and an env var, redeploy, and verify the app still serves.
7. Publish the app URL, verify external access, make it private, and verify the DNS record is removed.
8. Enable auto-redeploy, push to the selected branch, and verify the commit-specific deployment starts.
9. Roll back to a previous successful deployment.
10. Delete the app and verify managed containers, images, route snippets, DNS record, app data volume, resource snapshots, and DB rows are cleaned up.
11. Run `hostlet backup`, then restore into a clean stack with the original `.env`.

## Known Non-Blocking Limits

- Remote VPS agents are disabled.
- Deploy jobs are not backed by a durable queue.
- Delete finalization is not reconciled after API restart.
- Audit logging and RBAC are not implemented.
- Rate limits are in-memory only.
- Generated static Node apps assume `dist` and `npx serve`.
- Release assets are Linux x86_64 only unless the release workflow is expanded.
- Release signing, SBOMs, attestations, and image scanning are not yet part of the package.
