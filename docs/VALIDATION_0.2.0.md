# Hostlet 0.2.0 Validation Checklist

Date: 2026-05-24

Use this checklist before tagging `v0.2.0`. Automated checks can run in this repository; manual checks need an owner-controlled Hostlet host with Docker, valid `.env` values, and a disposable test app.

## Automated Gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
```

CLI smoke checks:

```bash
hostlet version
hostlet update check
hostlet update --dry-run
hostlet status
hostlet doctor
```

If local `target/` permissions are dirty, run CLI smoke checks with an isolated build target:

```bash
CARGO_TARGET_DIR=/tmp/hostlet-codex-target cargo run -p hostlet -- version
CARGO_TARGET_DIR=/tmp/hostlet-codex-target cargo run -p hostlet -- update check
```

## Manual Upgrade Validation

1. Install Hostlet `0.1.0` on a disposable host.
2. Deploy a test app with a working health path.
3. Confirm the app serves through its configured route.
4. Upgrade to `0.2.0` with `hostlet update`.
5. Confirm the app still serves after the update.
6. Confirm runtime health appears on the dashboard, app list, and app detail page without a browser refresh.
7. Use **Check now** and confirm a fresh health event appears.
8. Break the health path and wait for `degraded`, then `unhealthy`.
9. Restore the health path and confirm the app returns to `healthy`.
10. Use **Restart container** and confirm the agent records a post-restart health result.
11. Run `hostlet status` and confirm service, update, and app health summaries are accurate.
12. Run `hostlet doctor` and review warnings.
13. Run `hostlet backup`, restore into a clean disposable environment, and confirm Hostlet and the test app data recover.
14. Run `hostlet update rollback` in a disposable upgraded install and confirm the previous CLI and saved Compose files are restored.

## Release Artifact Validation

For the release workflow output, confirm:

- `hostlet-linux-x64` is executable.
- `hostlet-linux-x64.sha256` matches the binary.
- `hostlet-release.json` is valid JSON.
- manifest `version` matches the tag without the leading `v`.
- manifest `minimum_supported_version` is `0.1.0`.
- manifest checksum for `hostlet-linux-x64` matches `hostlet-linux-x64.sha256`.
- `hostlet update check` can parse the release with and without the manifest fallback path.

## Known Manual Gaps

These cannot be proven by repository-only checks:

- Real `0.1.0` to `0.2.0` upgrade on an owner-controlled host.
- Cloudflare DNS/tunnel behavior with production credentials.
- Full backup and restore with realistic app data.
- Release workflow execution on the self-hosted runner.
