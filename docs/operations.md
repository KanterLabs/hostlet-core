# Operations

This guide covers current operational workflows for self-hosted Hostlet and public-safe Hostlet Cloud operations.

## Self-Hosted Commands

```bash
hostlet status
hostlet logs
hostlet doctor
hostlet update check
hostlet update
hostlet backup
hostlet restore backups/hostlet-YYYYMMDDTHHMMSSZ
hostlet down
```

## Updates

Hostlet checks GitHub Releases for stable updates. When `hostlet-release.json` is present, Hostlet uses it for release version, minimum supported direct-upgrade version, checksums, image metadata, and migration flags.

Production deploys should use tagged release images:

```text
HOSTLET_IMAGE_TAG=vX.Y.Z
```

Then pull and restart with `--no-build`.

## Backup And Restore

Backups include a Postgres dump and agent state volume when available. They intentionally do not copy `.env`, `.env.prod`, raw secret values, private keys, or app environment values.

Restores require the original `ENCRYPTION_KEY`. Without it, encrypted GitHub tokens and app environment variables cannot be decrypted.

## Hostlet Cloud Release Checks

For public Hostlet Cloud checks:

```bash
curl -fsS https://hostlet.cloud/health
curl -fsSI https://hostlet.cloud/pricing
```

Also confirm:

- API, web, managed agent, Postgres, Caddy, and cloudflared are running.
- API, web, and managed agent use the intended `vX.Y.Z` images.
- raw app ports, Postgres, Docker, Caddy admin, and internal control surfaces are not exposed publicly.

Do not put exact production inventory, private VM paths, provider IDs, or secret values in tracked docs.

## Troubleshooting

- If API startup fails after an environment change, check Postgres credential compatibility with the existing persistent volume.
- If app routing fails, check generated Caddy snippets and Caddy reload logs.
- If deploy logs stop updating, check agent connectivity and job signing configuration.
- If GitHub webhooks are not firing, confirm `PUBLIC_WEBHOOK_URL` is public HTTPS and the webhook secret is configured.
