# Hostlet 0.3.10 Validation

## Automated

```bash
cargo fmt --all -- --check
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
docker build -f apps/agent/Dockerfile -t hostlet-agent-compose-test .
docker run --rm hostlet-agent-compose-test docker compose version
```

## Manual

1. Confirm `hostlet version` reports `0.3.10`.
2. Confirm the local agent container can run `docker compose version`.
3. Retry a Gitea deployment created through the public repo inspection flow.
4. Confirm deployment passes the `docker compose config` step and reaches health checking.
