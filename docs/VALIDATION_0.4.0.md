# Hostlet 0.4.0 Validation

Use this checklist before pushing implementation work and before tagging `v0.4.0`.

## Automated

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
HOSTLET_CADDYFILE=./Caddyfile.direct docker compose -f infra/docker-compose.prod.yml config
```

## Self-Hosted Regression

1. Confirm `hostlet version` reports `0.4.0`.
2. Run setup/unlock with `HOSTLET_MODE=self_hosted` or unset.
3. Connect GitHub through Device Flow.
4. Create and deploy a local app.
5. Confirm logs, runtime health, manual restart, rollback, publish/private URL, and delete still work.
6. Confirm local deploys do not require a Hostlet Cloud account.

## Cloud Foundation

1. Run the API with `HOSTLET_MODE=cloud`.
2. Confirm `/api/system/version` reports `mode: cloud`.
3. Confirm `/api/cloud/status` reports which GitHub App and Stripe env values are configured without exposing secrets.
4. Confirm migration `021_cloud_accounts.sql` creates cloud users, sessions, GitHub installations, Stripe records, entitlements, usage buckets, and webhook dedupe tables.
5. Confirm `HOSTLET_CADDYFILE=./Caddyfile.direct` validates through Compose.

## Direct-Origin Cloud Infra

1. Confirm `hostlet.cloud` points through Cloudflare to the reserved VM IP.
2. Confirm `*.hostlet.cloud` points through Cloudflare to the reserved VM IP.
3. Confirm GCP firewall allows ports 80 and 443 to the VM.
4. Confirm Caddy routes `hostlet.cloud` to the web/API services and wildcard app hostnames to managed app snippets.

## Release Gate

Do not push the implementation branch until local self-hosted and cloud-foundation validation pass.

Do not tag `v0.4.0` until CI passes on the pushed branch.
