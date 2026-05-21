# Hostlet

Hostlet is a small self-hosted deployment panel for GitHub projects. It runs a web UI, a Rust API, PostgreSQL, a local deployment agent, Caddy, and optional Cloudflare Tunnel support.

## Quick Setup

Requirements:

- Docker and Docker Compose
- A GitHub OAuth App
- Optional: a Cloudflare zone and tunnel token for public `*.your-domain.com` app URLs

1. Build the CLI and run the setup wizard:

```bash
cargo run -p hostlet -- init
```

The wizard generates `.env`, asks for GitHub OAuth values, optionally configures Cloudflare Tunnel values, and prints the first setup token.
In Cloudflare mode it also prepares the Hostlet control-plane hostname so the UI, API, OAuth callback, and webhooks can all work through the tunnel.

2. Create a GitHub OAuth App when prompted.

For local/LAN testing, set:

```text
Homepage URL: http://YOUR_HOST_IP:3000
Authorization callback URL: http://YOUR_HOST_IP:8080/auth/github/callback
```

For Cloudflare Tunnel mode, use the HTTPS Hostlet hostname that `hostlet init` asks for:

```text
Homepage URL: https://hostlet.example.com
Authorization callback URL: https://hostlet.example.com/auth/github/callback
```

3. Start Hostlet:

```bash
cargo run -p hostlet -- up
```

For Cloudflare Tunnel mode:

```bash
cargo run -p hostlet -- up --tunnel
```

4. Open the UI printed by `hostlet init`.

Manual setup is still supported with:

```bash
cp .env.example .env
```

Set the first-run password, unlock the panel, sign in with GitHub, create an app, and deploy it to `This machine`.

If `HOSTLET_SETUP_TOKEN` is set, paste it into the first-run setup form.

## Optional Public App URLs

Hostlet can publish individual apps through Cloudflare Tunnel. Configure these in `.env`:

```bash
HOSTLET_BASE_DOMAIN=example.com
HOSTLET_DOMAIN_PREFIX=hostlet-
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ZONE_ID=...
CLOUDFLARE_TUNNEL_TARGET=<tunnel-id>.cfargotunnel.com
CLOUDFLARE_TUNNEL_TOKEN=...
```

Apps are private by default. Use **Open tunnel** or **Close tunnel** on the app detail page to add or remove the app DNS record.

## Auto-Redeploy

Auto-redeploy is off by default. Enable **Auto redeploy on branch push** for an app, then add a GitHub repository webhook:

```text
Payload URL: PUBLIC_API_URL/webhooks/github
Content type: application/json
Secret: GITHUB_WEBHOOK_SECRET
Events: push
```

Only matching repo/branch pushes for apps with auto-redeploy enabled start deployments.

## Backup

```bash
scripts/backup.sh
HOSTLET_RESTORE_CONFIRM=yes scripts/restore.sh backups/hostlet-YYYYMMDDTHHMMSSZ
```

Keep `.env` secrets in a password manager. The backup intentionally stores a checklist, not live secret values.

## Production Compose

Development uses `infra/docker-compose.yml`. Production builds images and avoids source bind mounts:

```bash
docker compose -f infra/docker-compose.prod.yml up -d --build
```

Add `--profile tunnel` when running Cloudflare Tunnel from the same host.

## More Docs

- [Full guide](docs/README.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Security](docs/SECURITY.md)
- [Missing feature report](docs/FEATURE_GAPS.md)
