# Hostlet Documentation

This guide documents the current Hostlet implementation: local setup, Cloudflare tunnel setup, app deployment, webhooks, remote agents, operations, and known limits.

## Components

- `apps/web`: Next.js dashboard on port `3000`.
- `apps/api`: Rust Axum API on port `8080`.
- `apps/agent`: Rust deployment agent. The local agent runs with Docker access and builds/runs apps.
- `postgres`: control-plane database.
- `hostlet-caddy`: local reverse proxy on loopback port `18080` for tunnel traffic.
- `cloudflared`: optional Cloudflare Tunnel connector.

## Recommended Setup With CLI

1. Install Docker and Docker Compose.

2. Build and run the setup wizard:

```bash
cargo run -p hostlet -- init
```

The wizard asks for:

- public access mode: LAN only or Cloudflare Tunnel
- LAN host/IP or Cloudflare domain settings
- GitHub OAuth Client ID and secret
- allowed GitHub username
- Hostlet repository URL for remote agent installs

It generates all required secrets, writes `.env`, and prints the first setup token. In Cloudflare mode it also validates the zone token and can create/update the Hostlet UI/API CNAME record pointing at the configured tunnel target.

3. Start Hostlet:

```bash
cargo run -p hostlet -- up
```

For Cloudflare Tunnel mode:

```bash
cargo run -p hostlet -- up --tunnel
```

4. Check the install:

```bash
cargo run -p hostlet -- doctor
```

Operational commands:

```bash
cargo run -p hostlet -- logs
cargo run -p hostlet -- backup
cargo run -p hostlet -- restore backups/hostlet-YYYYMMDDTHHMMSSZ
cargo run -p hostlet -- down
```

## Manual Local/LAN Setup

1. Install Docker and Docker Compose.

2. Copy the example environment:

```bash
cp .env.example .env
```

3. Set your LAN URLs. Replace `10.0.0.194` with the machine running Hostlet:

```bash
PUBLIC_WEB_URL=http://10.0.0.194:3000
PUBLIC_API_URL=http://10.0.0.194:8080
HOSTLET_CONTROL_PLANE_HOST=10.0.0.194
HOSTLET_ALLOWED_WEB_ORIGINS=http://10.0.0.194:3000,http://localhost:3000,http://127.0.0.1:3000
```

4. Create a GitHub OAuth App:

```text
Homepage URL: http://10.0.0.194:3000
Authorization callback URL: http://10.0.0.194:8080/auth/github/callback
```

5. Add the OAuth values:

```bash
GITHUB_CLIENT_ID=...
GITHUB_CLIENT_SECRET=...
```

6. Generate secrets:

```bash
openssl rand -base64 32 # ENCRYPTION_KEY
openssl rand -hex 24    # POSTGRES_PASSWORD
openssl rand -hex 32    # SESSION_SECRET
openssl rand -hex 32    # JOB_SIGNING_SECRET
openssl rand -hex 32    # LOCAL_AGENT_TOKEN
openssl rand -hex 32    # GITHUB_WEBHOOK_SECRET
openssl rand -hex 32    # HOSTLET_SETUP_TOKEN
```

7. Start the stack:

```bash
docker compose -f infra/docker-compose.yml up -d
```

8. Open:

```text
http://10.0.0.194:3000
```

On first run, Hostlet asks you to set a control-plane password. If `HOSTLET_SETUP_TOKEN` is set, paste it into the setup-token field. After setup, unlock the panel and continue with GitHub.

## GitHub OAuth

Hostlet uses GitHub OAuth for dashboard login and repository listing. The callback URL must match `PUBLIC_API_URL` exactly:

```text
PUBLIC_API_URL/auth/github/callback
```

Common examples:

```text
http://localhost:8080/auth/github/callback
http://10.0.0.194:8080/auth/github/callback
https://api.hostlet.example.com/auth/github/callback
```

If GitHub shows `redirect_uri is not associated with this application`, update the OAuth App callback URL to the exact URL Hostlet is using.

## First-Run Password

The control-plane password is separate from GitHub. It protects the panel before OAuth and is required on every browser session before using the UI.

In secure mode, set `HOSTLET_SETUP_TOKEN`. First setup requests must include that token in the setup-token field. After the password is set, the setup token is no longer used for normal unlocks.

## Creating and Deploying an App

1. Open **Apps**.
2. Click **Create app**.
3. Select a GitHub repository or paste `owner/repo`.
4. Choose branch, app name, runtime settings, resource limits, and deploy target.
5. Leave **Expose through tunnel** unchecked unless the app should be public immediately.
6. Leave **Auto redeploy on branch push** unchecked unless pushes to this branch should deploy automatically.
7. Click **Create app**.
8. Open the app detail page and click **Deploy**.

Hostlet deploys by sending a signed job to the selected agent. The agent clones or updates the repository, builds a Docker image, starts a container, health-checks it, and updates local routing after the health check passes.

## Build Detection

If the repository contains a `Dockerfile`, Hostlet uses it.

If no `Dockerfile` exists, Hostlet looks for `package.json` and generates a Node Dockerfile. It detects common Node frameworks and package managers:

- Next.js
- Vite
- Astro
- Nuxt
- Remix
- SvelteKit
- generic Node
- npm, pnpm, or yarn

You can override root directory, install command, build command, and start command from the create-app form.

## App Settings and Environment Variables

Each app detail page includes editable settings for:

- domain and health path
- root directory
- install, build, and start commands
- container port
- memory and CPU limits
- public tunnel state
- auto-redeploy state

Runtime changes require a redeploy before they affect the running container.

Environment variables are stored encrypted. The UI lists keys only and never displays decrypted values. To change a value, enter a replacement value and save it, then redeploy the app.

## Public Tunnel Exposure

Public exposure is optional and per app.

Required `.env` values:

```bash
HOSTLET_BASE_DOMAIN=example.com
HOSTLET_DOMAIN_PREFIX=hostlet-
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ZONE_ID=...
CLOUDFLARE_TUNNEL_TARGET=<tunnel-id>.cfargotunnel.com
CLOUDFLARE_TUNNEL_TOKEN=...
```

Behavior:

- New apps default to private.
- **Open tunnel** creates or updates a proxied Cloudflare CNAME/Tunnel record for the app hostname.
- **Close tunnel** deletes the app hostname record.
- Hostlet only manages single-label hostnames that start with `HOSTLET_DOMAIN_PREFIX` under `HOSTLET_BASE_DOMAIN`.
- Existing unrelated records, including the apex portfolio site, are not managed by Hostlet.

Cloudflare Tunnel ingress is wildcard-based:

```text
hostlet.example.com -> http://127.0.0.1:18080
*.example.com -> http://127.0.0.1:18080
```

Caddy routes the Hostlet control-plane hostname to the web/API services and routes app hostnames to their local container ports.

## GitHub Webhooks

Hostlet accepts GitHub push webhooks at:

```text
PUBLIC_API_URL/webhooks/github
```

Configure the repository webhook manually:

- Content type: `application/json`
- Secret: `GITHUB_WEBHOOK_SECRET`
- Events: push

When a push event matches an app repository and branch, Hostlet creates a deployment for that exact commit SHA only if **Auto redeploy on branch push** is enabled for that app. Webhook deliveries are deduplicated by GitHub delivery ID and the app detail page shows the latest webhook result.

## Remote VPS Agents

The UI can create a remote server install token:

1. Open **Machines**.
2. Click **Add VPS**.
3. Enter a name and optional public IP.
4. Run the generated install command on the VPS.

The install script installs Docker, Caddy, Rust tooling, builds `hostlet-agent`, registers it with the API, and installs a systemd service.

The generated install command includes `HOSTLET_REPO_URL` when `HOSTLET_REPO_URL` is configured for the API. If the command contains `REPLACE_WITH_HOSTLET_REPO_URL`, replace it with the Git URL for this Hostlet repository before running it.

## Runtime and Resource Limits

Deployed containers run with:

- `--security-opt no-new-privileges`
- `--cap-drop ALL`
- `--pids-limit 256`
- loopback-only published port
- optional memory and CPU limits
- `--read-only` and `/tmp` tmpfs for user-provided Dockerfiles

The local agent publishes Docker resource samples every 5 seconds. The app detail page shows current CPU, memory, network, disk, and process usage for local apps.

## Logs

Deployment logs are stored in PostgreSQL and streamed over a WebSocket while a deployment is running.

Limits:

- Max log line stored: 8 KiB
- Max logs per deployment: 20,000
- Deployment page shows the latest 1,000 lines in the browser

## Rollback

Rollback finds the previous successful deployment for the app and routes traffic back to that container. If routing fails, Hostlet preserves the current working app.

Rollback currently changes routing only. It does not remove newer containers or reconcile application data.

## Backup and Restore

Create a local backup:

```bash
scripts/backup.sh
```

Restore into a running stack:

```bash
HOSTLET_RESTORE_CONFIRM=yes scripts/restore.sh backups/hostlet-YYYYMMDDTHHMMSSZ
```

Backups include a Postgres dump and, when available, the local agent state volume. They intentionally do not copy `.env` because it contains live secrets. Restores require the original `ENCRYPTION_KEY` to decrypt GitHub tokens and app environment variables.

## Production Compose

Build and run production images:

```bash
docker compose -f infra/docker-compose.prod.yml up -d --build
```

Run Cloudflare Tunnel from the same Compose stack only when needed:

```bash
docker compose -f infra/docker-compose.prod.yml --profile tunnel up -d --build
```

Production Compose uses release Dockerfiles for the API, web UI, and local agent. It does not bind-mount the source tree.
The API and web services bind to loopback by default; Caddy/cloudflared are the public ingress when tunnel mode is enabled.

## Useful Commands

Start or update the stack:

```bash
docker compose -f infra/docker-compose.yml up -d
```

Recreate API and web:

```bash
docker compose -f infra/docker-compose.yml up -d --force-recreate api web
```

View logs:

```bash
docker compose -f infra/docker-compose.yml logs -f api web local-agent
```

Check health:

```bash
curl -fsS http://127.0.0.1:8080/health
curl -I http://127.0.0.1:3000
```

Check public DNS:

```bash
dig app.example.com @1.1.1.1
curl -I https://app.example.com/
```

Run checks:

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm --dir apps/web lint
```

## Production Checklist

- Set `HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false`.
- Use unique high-entropy secrets.
- Set `HOSTLET_ALLOWED_GITHUB_LOGINS`.
- Put the web and API behind HTTPS.
- Use a non-default Postgres password.
- Back up Postgres and `/var/lib/hostlet`.
- Restrict host firewall access.
- Keep Docker, Caddy, cloudflared, and base images patched.
- Review [Security](SECURITY.md) before internet exposure.
