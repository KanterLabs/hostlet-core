# Hostlet 0.3.11 Validation

## Automated

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

## Manual

1. Confirm `hostlet version` reports `0.3.11`.
2. Set `HOSTLET_PRIVATE_APP_HOST` to the machine address used by browsers.
3. Redeploy a private app and confirm Docker publishes the app port on `0.0.0.0`.
4. Open the app from the Visit action in the apps list and app detail page.

