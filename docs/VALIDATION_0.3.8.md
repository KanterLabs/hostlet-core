# Hostlet 0.3.8 Validation

Run before publishing:

```bash
cargo test
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

Manual checks:

1. Deploy an existing Dockerfile app and confirm deploy, health, restart, rollback, and delete still work.
2. Deploy a generated Node app and confirm no `hostlet.yml` is required.
3. Deploy a Compose app with `web` plus Redis and confirm route, health, logs, runtime stats, restart, rollback, and delete cleanup.
4. Deploy a Compose app with a named Postgres volume, redeploy changed web code, and confirm data persists.
5. Try a Compose file with `ports`, `container_name`, host networking, and a host bind mount; each should fail before deployment.
6. Enable auto-redeploy on a Compose app and confirm a matching push deploys the webhook commit.
