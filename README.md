# Hostlet

Hostlet is a small self-hosted deployment panel for GitHub projects. It runs a web UI, a Rust API, PostgreSQL, a local deployment agent, Caddy, and optional Cloudflare Tunnel support.

## Quick Setup

Requirements:

- Docker and Docker Compose
- Git and curl
- A GitHub OAuth App with Device Flow enabled
- Optional: a Cloudflare zone and tunnel token for public `*.your-domain.com` app URLs

1. Clone Hostlet and install the compiled CLI:

```bash
git clone https://github.com/ShaneKanterman04/Hostlet.git
cd Hostlet
curl -L https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/hostlet-linux-x64 -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

2. Run the setup wizard:

```bash
hostlet init
```

The wizard generates `.env`, asks for the GitHub OAuth Client ID, optionally configures Cloudflare Tunnel values, and prints the first setup token.

Access modes are:

- **LAN only**: the Hostlet UI runs at `http://YOUR_HOST_IP:3000` and the API at `http://YOUR_HOST_IP:8080`. Manual deploys work, but GitHub cannot send webhooks to a private LAN URL.
- **Cloudflare Tunnel for Hostlet UI/API**: the Hostlet UI, API, and webhooks share one HTTPS hostname through Cloudflare Tunnel.

Deployed apps are still private by default in both modes. Making an app public is a separate per-app action.

3. Create a GitHub OAuth App when prompted.

Enable **Device Flow** on the OAuth App and copy the Client ID into Hostlet. Hostlet does not use an OAuth callback URL or client secret.

For local/LAN testing, set:

```text
Homepage URL: http://YOUR_HOST_IP:3000
```

For Cloudflare Tunnel UI/API mode, use the HTTPS Hostlet hostname that `hostlet init` asks for:

```text
Homepage URL: https://hostlet.example.com
```

4. Start Hostlet:

```bash
hostlet up
```

For Cloudflare Tunnel UI/API mode:

```bash
hostlet up --tunnel
```

5. Open the UI printed by `hostlet init`.

Developers can run the CLI from source with `cargo run -p hostlet -- <command>`, but production installs should use the compiled release binary.

Manual setup is still supported with:

```bash
cp .env.example .env
```

Set the first-run password, unlock the panel, sign in with GitHub, create an app, and deploy it to `This machine`.

If `HOSTLET_SETUP_TOKEN` is set, paste it into the first-run setup form.

In LAN mode, deploy changes manually:

1. Push your app changes to GitHub.
2. Open the app in Hostlet.
3. Click **Deploy latest**.

Hostlet will pull the configured repo/branch and deploy the newest commit. Use Cloudflare Tunnel UI/API mode, or set a separate `PUBLIC_WEBHOOK_URL`, if you want GitHub pushes to deploy automatically.

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

Apps are private by default. Use **Publish URL** or **Make private** on the app detail page to add or remove the app DNS record.

## Auto-Redeploy

Auto-redeploy is off by default and only works when GitHub can reach Hostlet from the internet.

In full tunnel mode, `PUBLIC_API_URL` is the public HTTPS control-plane URL:

```text
PUBLIC_API_URL=https://hostlet.example.com
```

If you keep the UI/API in LAN mode but still expose only the webhook endpoint through a tunnel, leave `PUBLIC_API_URL` on the LAN URL and set:

```text
PUBLIC_WEBHOOK_URL=https://hostlet.example.com
```

These LAN/local values are valid for Device Flow sign-in and manual deploys, but not for GitHub webhook delivery unless `PUBLIC_WEBHOOK_URL` points at a public HTTPS tunnel:

```text
PUBLIC_API_URL=http://localhost:8080
PUBLIC_API_URL=http://10.0.0.194:8080
PUBLIC_API_URL=http://192.168.1.20:8080
```

To enable auto-redeploy:

1. Run Hostlet with a public control-plane URL, usually `hostlet up --tunnel`, or set `PUBLIC_WEBHOOK_URL` to a public HTTPS tunnel hostname.
2. Enable **Auto redeploy on branch push** for the app.
3. Add a GitHub repository webhook:

```text
Payload URL: PUBLIC_WEBHOOK_URL/webhooks/github
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
