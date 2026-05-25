# Hostlet 0.3.0 Validation Checklist

Date: 2026-05-25

Use this checklist before tagging `v0.3.0`.

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

Compose config requires production-like environment variables. For syntax-only validation, use disposable values for the required secrets.

## Durable Job Validation

1. Trigger **Check now** on an app and confirm an `agent_jobs` row is queued, claimed, then completed.
2. Trigger **Restart container** and confirm the same durable job lifecycle.
3. Stop the local agent with a claimed job and confirm the API requeues it after the lease expires.
4. Exhaust retry attempts for a claimed job and confirm it is marked `failed`.
5. Confirm claimed jobs are signed and rejected by the agent if the signature is invalid.

## Audit Validation

1. Trigger **Check now** and confirm a `health_check_requested` audit event.
2. Trigger **Restart container** and confirm a `restart_container_requested` audit event.
3. Call `/api/audit-events` as the owner and confirm recent events are returned without secret values.

## Release Artifact Validation

1. Confirm `hostlet-linux-x64` is executable.
2. Confirm `hostlet-linux-x64.sha256` matches the binary.
3. Confirm `hostlet-release.json` is valid JSON and matches the tag version.
4. Confirm `hostlet-linux-x64.spdx.json` is uploaded.
5. Confirm GitHub artifact attestations exist for the release assets.
6. If signing secrets are configured, confirm `hostlet-linux-x64.sha256.asc` verifies.

## Production Update Validation

1. Run a pre-update backup from `/home/shane/hostlet-release`.
2. Rebuild and restart the production Compose stack.
3. Confirm `/health` returns `ok`.
4. Confirm `hostlet version` reports `0.3.0`.
5. Confirm the local agent is online.
6. Confirm existing apps still serve.
7. Run `hostlet status` and `hostlet doctor`.
