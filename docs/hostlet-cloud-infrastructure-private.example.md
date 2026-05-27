# Hostlet Cloud Infrastructure Private Tracker Example

Copy this template to:

```text
docs/private/hostlet-cloud-infrastructure.md
```

The destination path is gitignored. Do not store raw secret values here. Use secret names, presence state, rotation dates, and references to the system that stores the secret.

## Production Target

- VM/provider name:
- Hostname:
- Internal access method:
- Install path:
- Compose project:
- Environment file path:
- Current release tag:
- Current release commit:
- Last verified:

## Public Surface

- Control-plane domain:
- Managed app wildcard:
- Health check:
- Pricing check:
- Cloudflare zone/account references:
- Tunnel reference:

## Runtime Services

| Service | Container | Image/tag | Notes |
| --- | --- | --- | --- |
| API | | | |
| Web | | | |
| Managed agent | | | |
| Postgres | | | |
| Caddy | | | |
| cloudflared | | | |

## Required Secret Keys

Record only status and storage references. Do not paste values.

| Key | Status | Stored in | Last rotated | Notes |
| --- | --- | --- | --- | --- |
| `POSTGRES_PASSWORD` | | | | |
| `ENCRYPTION_KEY` | | | | |
| `SESSION_SECRET` | | | | |
| `JOB_SIGNING_SECRET` | | | | |
| `LOCAL_AGENT_TOKEN` | | | | |
| `HOSTLET_SETUP_TOKEN` | | | | |
| `GITHUB_WEBHOOK_SECRET` | | | | |
| `GITHUB_APP_PRIVATE_KEY_PEM` | | | | |
| `GITHUB_APP_WEBHOOK_SECRET` | | | | |
| `STRIPE_SECRET_KEY` | | | | |
| `STRIPE_WEBHOOK_SECRET` | | | | |
| `CLOUDFLARE_API_TOKEN` | | | | |
| `CLOUDFLARE_TUNNEL_TOKEN` | | | | |

## Backups And Rollback

- Latest pre-cutover backup:
- Latest old release directory:
- Persistent volumes:
- Rollback command notes:
- Last restore test:

## Verification Log

| Date | Release | Checks | Result | Notes |
| --- | --- | --- | --- | --- |
| | | health, pricing, services, images | | |

## Incident Log

| Date | Incident | Impact | Fix | Follow-up |
| --- | --- | --- | --- | --- |
| | | | | |
