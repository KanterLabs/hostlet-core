# Contributing

Hostlet Core is open source self-hosted infrastructure. Contributions should preserve self-hosted behavior and keep hosted-service code out of the public repo.

## Local Checks

Run relevant checks before opening a change:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
HOSTLET_IMAGE_TAG=v0.0.0 docker compose -f infra/docker-compose.prod.yml config
```

Use narrower checks for small docs-only changes, but always run link and secret scans for documentation edits.

## Docs Rules

- Keep docs plain Markdown.
- Use **Hostlet** or **Hostlet Core** for the open-source self-hostable product.
- Do not add historical plans or versioned validation files back into `docs/`.
- Do not document secret values, internal-only IPs, provider IDs, private backup paths, billing config, private deployment config, or raw env files in tracked docs.
- Keep hosted-service production inventory in the private hosted-service repo, not here.

## Release Expectations

Production releases are tagged `vX.Y.Z` and publish:

- CLI binary and checksum
- `hostlet-release.json`
- GHCR images for API, web, and agent

## Security Review Expectations

Review carefully when touching:

- auth and session handling
- GitHub OAuth code
- encryption and secret handling
- API-to-agent job signing
- Docker/Caddy agent code
- database migrations
