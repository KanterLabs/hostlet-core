# Contributing

Hostlet is open source and also powers Hostlet Cloud. Contributions should preserve both self-hosted behavior and cloud-mode safety.

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
- Use **Hostlet** for the open-source self-hostable product.
- Use **Hostlet Cloud** for the managed SaaS app-hosting service.
- Do not add historical plans or versioned validation files back into `docs/`.
- Do not document secret values, internal-only IPs, provider IDs, private backup paths, or raw env files in tracked docs.
- Keep exact Hostlet Cloud production inventory in `docs/private/`, which is ignored.

## Release Expectations

Production releases are tagged `vX.Y.Z` and publish:

- CLI binary and checksum
- `hostlet-release.json`
- GHCR images for API, web, and agent

Hostlet Cloud and self-hosted production should consume the same tagged release images.

## Security Review Expectations

Review carefully when touching:

- auth and session handling
- GitHub OAuth/App code
- Stripe billing and webhooks
- encryption and secret handling
- API-to-agent job signing
- Docker/Caddy agent code
- database migrations that affect tenancy or billing
