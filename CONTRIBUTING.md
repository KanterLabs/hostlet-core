# Contributing

Hostlet Core is the public, self-hosted Hostlet project. Keep hosted-service billing, private provider configuration, company infrastructure, and customer-specific logic out of this repo.

## Branches & releases

- **`staging`** is the development branch — open PRs against it (or push directly if you own the change). Pushing `staging` runs `.github/workflows/staging.yml`: quality gates, then it builds and publishes the moving `hostlet-{api,web,agent}:staging` (+ `:sha-<commit>`) images.
- **`main`** is the release branch. Releases are tags `vX.Y.Z`; the tag must match the `version` in `apps/cli/Cargo.toml`, `apps/api/Cargo.toml`, and `apps/agent/Cargo.toml` (bump all three before tagging or `release.yml` fails).
- See `AGENTS.md` for the short agent-facing version.

Run the relevant checks before opening a pull request:

```bash
scripts/validate-local.sh
```

For narrower local runs, use the matching commands directly:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
HOSTLET_IMAGE_TAG=v0.0.0 docker compose -f infra/docker-compose.prod.yml config
```

Use narrower checks for docs-only changes, but still avoid adding secrets or private operations data to tracked files.
