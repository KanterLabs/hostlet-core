# Hostlet 0.3.9 Validation

## Automated

```bash
cargo fmt --all -- --check
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

## Manual

1. Confirm `hostlet version` reports `0.3.9`.
2. Paste `https://github.com/go-gitea/gitea` on the create app page.
3. Inspect the repo and confirm the preview shows Compose, port `3000`, health path `/`, rootless SQLite, named volumes, and the SSH warning.
4. Click **Create and deploy** and confirm Hostlet opens the deployment log page.
5. Confirm the Gitea HTTP route becomes healthy.
6. Redeploy and confirm Gitea data persists.
7. Inspect a public repo with a root `Dockerfile` and confirm Hostlet infers a port or shows an ambiguity warning.
8. Inspect a public repo without a root `Dockerfile` or `package.json` and confirm Hostlet shows a non-deployable preview.
