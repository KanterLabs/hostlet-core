# Operations

This guide covers current operational workflows for self-hosted Hostlet.

## Self-Hosted Commands

```bash
hostlet version
hostlet status
hostlet logs
hostlet doctor
hostlet update check
hostlet update --dry-run
hostlet update
hostlet update rollback
hostlet backup
hostlet backup --scheduled
hostlet restore backups/hostlet-YYYYMMDDTHHMMSSZ
hostlet cleanup --dry-run
hostlet cleanup --yes
hostlet down
```

## Updates

Hostlet checks GitHub Releases for stable updates. When `hostlet-release.json` is present, Hostlet uses it for release version, minimum supported direct-upgrade version, checksums, image metadata, and migration flags.

Production deploys should use immutable release image refs from `.env`:

```text
HOSTLET_API_IMAGE=ghcr.io/shanekanterman04/hostlet-api@sha256:...
HOSTLET_WEB_IMAGE=ghcr.io/shanekanterman04/hostlet-web@sha256:...
HOSTLET_AGENT_IMAGE=ghcr.io/shanekanterman04/hostlet-agent@sha256:...
HOSTLET_SCREENSHOTTER_IMAGE=ghcr.io/shanekanterman04/hostlet-screenshotter@sha256:...
```

Then pull and restart with `--no-build`.

## Backup And Restore

Backups include a Postgres dump and agent state volume when available. The dump
contains encrypted database rows, including encrypted GitHub tokens and app
environment variables. Backups intentionally do not copy `.env`, `.env.prod`,
raw secret values, private keys, or plaintext app environment files.

Restores require the original `ENCRYPTION_KEY`. Without it, encrypted GitHub tokens and app environment variables cannot be decrypted.

`scripts/backup.sh` can also push the snapshot off-host: set `HOSTLET_BACKUP_BUCKET`
(a `gs://` path) to sync via `gsutil`, or `HOSTLET_BACKUP_S3_BUCKET` (an `s3://` path,
optionally with `HOSTLET_BACKUP_S3_ENDPOINT` for a non-AWS S3-compatible endpoint such as
Cloudflare R2 or MinIO) to sync via the `aws` CLI. Both are no-ops when unset; set at most
one. S3-compatible credentials/region come from the standard `AWS_ACCESS_KEY_ID` /
`AWS_SECRET_ACCESS_KEY` / `AWS_DEFAULT_REGION` environment variables — `hostlet backup`
inherits your shell's environment, so exporting these before running it (or in a cron/
systemd unit's environment) is enough; nothing needs to be set in `.env`.

## Troubleshooting

- If API startup fails after an environment change, check Postgres credential compatibility with the existing persistent volume.
- If app routing fails, check generated Caddy snippets and Caddy reload logs.
- If deploy logs stop updating, check agent connectivity and job signing configuration.
- If GitHub webhooks are not firing, confirm `PUBLIC_WEBHOOK_URL` is public HTTPS and the webhook secret is configured. LAN, private, or non-HTTPS URLs require manual deploy after a push; app detail surfaces this readiness state and shows a manual deploy action when available.

## Settings UI

The Settings page shows GitHub and Cloudflare connection status, update status,
latest backup metadata, cleanup preview/run controls, recent agent jobs with
retry/cancel actions, and a recent audit trail.
