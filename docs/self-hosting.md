# Self-Hosting Hostlet

Self-hosted Hostlet runs the web UI, API, Postgres, local agent, and Caddy router on your machine.

## Access Modes

LAN-only mode keeps the control plane on your local network:

```text
PUBLIC_WEB_URL=http://SERVER_IP:3000
PUBLIC_API_URL=http://SERVER_IP:8080
```

Cloudflare Tunnel mode exposes the Hostlet UI/API/webhooks through one HTTPS hostname:

```text
PUBLIC_WEB_URL=https://hostlet.example.com
PUBLIC_API_URL=https://hostlet.example.com
PUBLIC_WEBHOOK_URL=https://hostlet.example.com
```

These modes describe access to Hostlet itself. Apps are private by default and are exposed per app through Hostlet routing controls.

## GitHub Auth

Self-hosted Hostlet uses GitHub OAuth Device Flow.

Configure a GitHub OAuth App with Device Flow enabled and set:

```text
GITHUB_CLIENT_ID=<client id>
```

No callback URL or OAuth client secret is required for self-hosted Device Flow.

## First-Run Security

Hostlet uses:

- first-run setup token
- control-plane password
- unlock cookie
- GitHub account allowlist
- encrypted app environment variables

Set strong values for production secrets and keep `.env` out of git.

## Production Compose

Production Compose is image-only. It pulls tagged release images and starts with `--no-build`.

Set a release tag:

```text
HOSTLET_IMAGE_TAG=vX.Y.Z
```

Start production:

```bash
docker compose --env-file .env -f infra/docker-compose.prod.yml -p hostlet-release up -d --no-build
```

With tunnel profile:

```bash
docker compose --env-file .env -f infra/docker-compose.prod.yml --profile tunnel -p hostlet-release up -d --no-build
```

Use the same tagged release images for self-hosted production and Hostlet Cloud.

## Public App URLs

Public app exposure should go through Caddy and Cloudflare Tunnel or another trusted reverse proxy. Raw Docker app ports bind to loopback and should not be exposed directly.

Hostlet only manages Cloudflare records under the configured base domain and only for app-owned records.
