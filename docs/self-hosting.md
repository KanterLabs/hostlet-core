# Self-Hosting Hostlet

Self-hosted Hostlet runs the web UI, API, Postgres, local agent, and Caddy
router on one machine. The supported stable host target is Linux x86_64 with
glibc 2.39 or newer (for example, Ubuntu 24.04 LTS or newer), Docker Engine,
and Docker Compose v2. Alpine/musl and older glibc hosts are unsupported by the
current CLI binary.

The Machines page reports this local deploy target, including agent heartbeat
and deployment mode. Remote VPS management is not active in the current Core UI.

Core does not provide multi-host scheduling, high availability, automatic
failover, or a formal capacity/SLA target. Builds, app containers, Postgres,
and the control plane share the host, so size and monitor it for the workloads
you choose to run.

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

## Network Ports

The image-only Compose file keeps the API on `127.0.0.1:8080` and the web UI on
`127.0.0.1:3000`. Postgres listens on port `5432` only inside the Compose
network. App containers use dynamic loopback ports behind Caddy rather than
fixed public host ports.

| Access mode | Host ports | Network requirement |
| --- | --- | --- |
| LAN | TCP `HOSTLET_LAN_PORT` (default `80`) on the IPv4 address selected during `hostlet init` | Allow that port only from the intended LAN. |
| Cloudflare Tunnel | Caddy listens on loopback TCP `18080`; no public inbound app/control-plane port is required | `cloudflared` must be able to connect outbound to Cloudflare. |
| Direct public (manual, not turnkey) | TCP `80` and `443` | Public DNS must resolve to the host. Wildcard app TLS also requires a DNS-01-capable custom Caddy build/provider or a mounted wildcard certificate. |

Do not expose loopback ports `3000`, `8080`, dynamic app ports, the Docker
socket, or Postgres directly. The development Compose file is intentionally
different: it publishes the web UI and API for local development and binds
Postgres to loopback.

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

## Image-Only Compose

The production-named Compose profile is image-only. “Production” describes the
packaging mode, not a maturity, support, or uptime guarantee. It pulls release
images by immutable digest and starts with `--no-build`.

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

Direct public hosting is an advanced template, not a turnkey path. The stock
`caddy:2-alpine` image can obtain a normal control-plane certificate through
HTTP-01, but it cannot obtain the wildcard certificate required by
`*.HOSTLET_APPS_HOST`: ACME wildcard certificates require DNS-01, and the
stock image does not include DNS provider modules.

Before using `Caddyfile.direct`, provide one of:

- a custom Caddy image containing the correct DNS provider module, corresponding
  least-privilege DNS credentials, and an explicit `tls { dns ... }` policy; or
- a pre-provisioned wildcard certificate and key mounted into Caddy with an
  explicit `tls <cert> <key>` policy.

Then set `HOSTLET_CADDYFILE=./Caddyfile.direct` and real DNS names for
`HOSTLET_CONTROL_PLANE_HOST` and `HOSTLET_BASE_DOMAIN`. The Core CLI and Compose
file do not build the provider-enabled image, mount certificates, or configure
DNS-01 for you. Use Cloudflare Tunnel unless you deliberately supply and
operate this TLS setup.

## Public App URLs

Public app exposure should go through Caddy and Cloudflare Tunnel or another trusted reverse proxy. Raw Docker app ports bind to loopback and should not be exposed directly.

Hostlet only manages Cloudflare records under the configured base domain and only for app-owned records.
In Cloudflare Tunnel mode, users enter a one-label app subdomain and Hostlet expands it under the configured zone (for example, `notes` becomes `notes.example.com`). The setup token must have permission to read the zone, edit DNS, and manage Cloudflare Tunnels for the owning account. Hostlet creates a dedicated tunnel and stores the infrastructure token in the mode-0600 `.env`; application and GitHub tokens remain encrypted in the database.
