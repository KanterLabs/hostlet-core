# Contributing

Hostlet Core is the public, self-hosted Hostlet project. Keep hosted-service billing, private provider configuration, company infrastructure, and customer-specific logic out of this repo.

## Branches & releases

- **`staging`** is the development branch — open PRs against it (or push directly if you own the change). Pushing `staging` runs `.github/workflows/staging.yml`: quality gates, then it builds and publishes the moving `hostlet-{api,web,agent,screenshotter}:staging` (+ `:sha-<commit>`) images.
- **`main`** is the release branch. Releases are tags `vX.Y.Z`; the tag must match the `version` in `apps/cli/Cargo.toml`, `apps/api/Cargo.toml`, and `apps/agent/Cargo.toml` (bump all three before tagging or `release.yml` fails).
- See `AGENTS.md` for the short agent-facing version.

Run the relevant checks before opening a pull request:

```bash
scripts/validate-local.sh
```

The final `docker compose config` gate requires several env vars that have no defaults (compose fails with `${VAR:?Run hostlet init first}` when they are unset). Provide them by one of:

- **Shell-sourced `.env`** — `hostlet init` writes a repo-root `.env` (see `.env.example` for the full list); source it into your shell before running the script (`set -a; . ./.env; set +a`). Compose v2 auto-loads `.env` only from the directory of the first `-f` file (`infra/`), not from the cwd, so the repo-root file is **not** picked up automatically. The `hostlet` CLI now passes `--env-file .env` itself when the repo-root file exists, so this only affects direct `docker compose` invocations (like validate-local.sh).
- **CI-parity dummy values** — export the same env block used by the `compose` job in `.github/workflows/ci.yml`: `POSTGRES_PASSWORD`, `SESSION_SECRET`, `HOSTLET_SETUP_TOKEN`, `HOSTLET_ALLOWED_GITHUB_LOGINS`, `ENCRYPTION_KEY`, `JOB_SIGNING_SECRET`, `LOCAL_AGENT_TOKEN`, `GITHUB_WEBHOOK_SECRET`, `PUBLIC_API_URL`, `PUBLIC_WEB_URL`, `HOSTLET_IMAGE_TAG`, and `DOCKER_GID` (prod compose only).

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
