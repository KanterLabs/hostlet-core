# Self-Hosting Hostlet

Self-hosted Hostlet runs the web UI, API, Postgres, local agent, and Caddy router on your machine.

The Machines page reports this local deploy target, including agent heartbeat
and deployment mode. Remote VPS management is not active in the current Core UI.

## Access Modes

LAN mode serves the UI and API through one same-origin Caddy address on your local network:

```text
PUBLIC_WEB_URL=http://SERVER_IP
PUBLIC_API_URL=http://SERVER_IP
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

Production Compose is image-only. It pulls release images by immutable digest and starts with `--no-build`.

`hostlet init` and `hostlet update` write the release image refs into `.env`:

```text
HOSTLET_API_IMAGE=ghcr.io/shanekanterman04/hostlet-api@sha256:...
HOSTLET_WEB_IMAGE=ghcr.io/shanekanterman04/hostlet-web@sha256:...
HOSTLET_AGENT_IMAGE=ghcr.io/shanekanterman04/hostlet-agent@sha256:...
HOSTLET_SCREENSHOTTER_IMAGE=ghcr.io/shanekanterman04/hostlet-screenshotter@sha256:...
```

Run `hostlet preflight` first, then use `hostlet init` for a new installation. To change access mode, LAN address, GitHub allowlist, or Cloudflare settings later, use `hostlet configure`; it validates a candidate file and backs up `.env` before applying it.

Start production:

```bash
docker compose --project-name infra --env-file .env -f infra/docker-compose.prod.yml up -d --no-build
```

`hostlet up` supplies `--env-file .env` automatically when the repo-root file exists; the manual command above is for direct `docker compose` invocations.

With tunnel profile:

```bash
docker compose --project-name infra --env-file .env -f infra/docker-compose.prod.yml --profile tunnel up -d --no-build
```

Direct public hosting is an advanced, manual configuration. Set `HOSTLET_CADDYFILE=./Caddyfile.direct` and use
real DNS names for `HOSTLET_CONTROL_PLANE_HOST` and `HOSTLET_BASE_DOMAIN` so
Caddy can provision HTTPS certificates. The tunnel Caddyfile is the only mode
that intentionally serves plain HTTP on loopback.

## Public App URLs

Public app exposure should go through Caddy and Cloudflare Tunnel or another trusted reverse proxy. Raw Docker app ports bind to loopback and should not be exposed directly.

Hostlet only manages Cloudflare records under the configured base domain and only for app-owned records.
In Cloudflare Tunnel mode, users enter a one-label app subdomain and Hostlet expands it under the configured zone (for example, `notes` becomes `notes.example.com`). The setup token must have permission to read the zone, edit DNS, and manage Cloudflare Tunnels for the owning account. Hostlet creates a dedicated tunnel and stores the infrastructure token in the mode-0600 `.env`; application and GitHub tokens remain encrypted in the database.
