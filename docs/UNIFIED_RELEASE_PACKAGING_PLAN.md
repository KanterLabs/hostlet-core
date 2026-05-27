# Hostlet Unified Release Packaging Plan

## Summary

- Keep one `main` branch and one codebase for cloud and self-hosted Hostlet.
- Release both cloud and self-host from the same `vX.Y.Z` tag and the same GHCR image set.
- Treat `HOSTLET_MODE=cloud` vs `HOSTLET_MODE=self_hosted` as runtime configuration only: UI wording, billing/GitHub App gates, and Hostlet Cloud compute access differ, but features ship together.

## Key Changes

- Move GHCR publishing out of `main` CI and into the tag-based release workflow.
- On each `v*` tag, publish:
  - `ghcr.io/shanekanterman04/hostlet-api:vX.Y.Z`
  - `ghcr.io/shanekanterman04/hostlet-web:vX.Y.Z`
  - `ghcr.io/shanekanterman04/hostlet-agent:vX.Y.Z`
  - optional `sha-<commit>` aliases for traceability, created only from the same tag workflow.
- Stop using `cloud-prod` as the default deployment tag. Cloud production and self-hosted installs should both deploy `HOSTLET_IMAGE_TAG=vX.Y.Z`.
- Update `hostlet-release.json` to include the exact image refs and digests for API, web, and agent.
- Update `infra/docker-compose.prod.yml` so production uses prebuilt tagged images only, with `HOSTLET_IMAGE_TAG` required from `.env`.
- Update the CLI in `apps/cli/src/main.rs`:
  - `hostlet init` writes `HOSTLET_IMAGE_TAG=v<CARGO_PKG_VERSION>`.
  - `hostlet update` reads the release manifest, sets `.env` to the new release image tag, fetches/checks out the matching git tag, pulls images, and starts with `--no-build`.
  - dev mode remains local-build based.
- Update the cloud deploy script so `/srv/hostlet/.env` controls the exact release tag, and rollback means setting `HOSTLET_IMAGE_TAG` to a previous `vX.Y.Z`.

## Behavior

- `main` remains the shared development branch for both modes.
- Tags are the only deployable production release boundary.
- Hostlet Cloud runs the same tagged artifacts as self-hosted Hostlet.
- Cloud-only differences remain runtime gated by `HOSTLET_MODE=cloud`: hosted compute, billing, GitHub App install flow, and cloud-specific UI labels.
- Self-hosted keeps the same app/deploy/runtime feature surface, except it uses local Docker/Caddy compute instead of Hostlet Cloud compute.

## Test Plan

- Validate tag workflow builds CLI release assets and all three GHCR images for `vX.Y.Z`.
- Validate `docker compose -f infra/docker-compose.prod.yml config` requires and accepts `HOSTLET_IMAGE_TAG=vX.Y.Z` in both cloud and self-host envs.
- Add CLI tests for release manifest image parsing and `.env` image tag updates.
- Smoke test self-host update: old release to new release, image pull without local build, health check passes.
- Smoke test cloud deploy: set `HOSTLET_IMAGE_TAG=vX.Y.Z`, run deploy script, verify public `/health` and app routing.
- Keep existing web/API tests for `HOSTLET_MODE=cloud` and `self_hosted` UI/API behavior.

## Assumptions

- Chosen policy: cloud and self-host both promote from tagged releases.
- No separate cloud branch, no separate cloud package stream.
- `cloud-prod` is removed or left unused as a deprecated historical tag.
- SHA tags are acceptable only as traceability aliases, not as the normal cloud or self-host upgrade path.
