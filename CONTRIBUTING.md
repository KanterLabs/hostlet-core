# Contributing

Hostlet Core is the public, self-hosted Hostlet project. Keep hosted-service billing, private provider configuration, company infrastructure, and customer-specific logic out of this repo.

Run the relevant checks before opening a pull request:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
HOSTLET_IMAGE_TAG=v0.0.0 docker compose -f infra/docker-compose.prod.yml config
```

Use narrower checks for docs-only changes, but still avoid adding secrets or private operations data to tracked files.
