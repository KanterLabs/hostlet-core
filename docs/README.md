# Hostlet Documentation

This guide documents the current Hostlet implementation: local setup, Cloudflare tunnel setup, app deployment, webhooks, operations, and known limits.

Planning documents:

- [Hostlet 0.2.0 Plan](PLAN_0.2.0.md): recurring app health checks, live status refresh, update detection, easy CLI updates, and manual refresh work.
- [Hostlet 0.3.0 Plan](PLAN_0.3.0.md): durable agent jobs, recovery, audit history, retention, backups, release hardening, and remote-agent readiness.
- [Hostlet 0.3.0 Validation Checklist](VALIDATION_0.3.0.md): pre-tag validation for durable jobs, audit events, release artifacts, and production updates.

## Components

- `apps/web`: Next.js dashboard on port `3000`.
- `apps/api`: Rust Axum API on port `8080`.
- `apps/agent`: Rust deployment agent. The local agent runs with Docker access and builds/runs apps.
- `postgres`: control-plane database.
- `hostlet-caddy`: local reverse proxy on loopback port `18080` for tunnel traffic.
- `cloudflared`: optional Cloudflare Tunnel connector.

## Recommended Production Setup With CLI

1. Install Docker, Docker Compose, Git, and curl.

2. Clone Hostlet and install the compiled CLI:

```bash
git clone https://github.com/ShaneKanterman04/Hostlet.git
cd Hostlet
curl -L https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/hostlet-linux-x64 -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

3. Run the setup wizard:

```bash
hostlet init
```

The wizard asks for:

- Hostlet UI/API access mode: LAN only or Cloudflare Tunnel
- LAN host/IP or Cloudflare domain settings
- GitHub OAuth App Client ID with Device Flow enabled
- allowed GitHub username

It generates all required secrets, writes `.env`, and prints the first setup token. In Cloudflare Tunnel UI/API mode it also validates the zone token and can create/update the Hostlet UI/API CNAME record pointing at the configured tunnel target.

4. Start Hostlet:

```bash
hostlet up
```

For Cloudflare Tunnel UI/API mode:

```bash
hostlet up --tunnel
```

5. Check the install:

```bash
hostlet doctor
```

Operational commands:

```bash
hostlet logs
hostlet status
hostlet update check
hostlet update
hostlet backup
hostlet restore backups/hostlet-YYYYMMDDTHHMMSSZ
hostlet down
```

Developers can run the CLI from source with `cargo run -p hostlet -- <command>`, but production installs should use the compiled release binary.

## Access Modes

Hostlet has two control-plane access modes:

- **LAN only**: the Hostlet UI is opened on the local network, usually `http://HOST_IP:3000`, and the API is on `http://HOST_IP:8080`. Manual deploys work, but GitHub cannot send webhooks to a private LAN URL.
- **Cloudflare Tunnel for Hostlet UI/API**: the Hostlet UI, API, and webhooks share one HTTPS hostname such as `https://hostlet.example.com`.

These modes describe access to Hostlet itself. Deployed apps are private by default in both modes. Public app URLs are controlled per app with **Publish URL** / **Make private**.

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
```

Enable **Device Flow** in the OAuth App settings. Hostlet only needs the Client ID.

LAN mode deploy flow:

1. Push app changes to GitHub.
2. Open the app in Hostlet.
3. Click **Deploy latest**.

Hostlet pulls the configured repo/branch and deploys the newest commit. Auto-redeploy requires Cloudflare Tunnel UI/API mode, another public HTTPS control-plane URL, or a separate `PUBLIC_WEBHOOK_URL`.

5. Add the GitHub Device Flow value:

```bash
GITHUB_CLIENT_ID=...
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

## GitHub Device Flow

Hostlet uses GitHub OAuth Device Flow for dashboard login and repository listing. This avoids redirect URI setup for self-hosted LAN installs.

Create a GitHub OAuth App, enable **Device Flow**, and set:

```bash
GITHUB_CLIENT_ID=...
```

No `GITHUB_CLIENT_SECRET` or callback URL is required. In the UI, click **Connect GitHub**, open the GitHub verification page, and enter the displayed code.

## First-Run Password

The control-plane password is separate from GitHub. It protects the panel before GitHub login and is required on every browser session before using the UI.

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

## Runtime Health

After a successful deployment, the local agent keeps checking the current app container on its configured port and health path. Runtime health is separate from deployment status: a deployment can remain `success` while the running app later becomes `degraded` or `unhealthy`.

Health states:

- `healthy`: the container is running and the health URL returns HTTP `2xx` or `3xx`.
- `degraded`: the latest check failed, but the failure threshold has not been reached.
- `unhealthy`: three consecutive checks failed.
- `unknown`: no runtime health check has been recorded yet.

The app list, dashboard, and app detail page poll for fresh health data. On the app detail page, use **Check now** for an immediate probe or **Restart container** to manually restart the current running container. Automatic restart and rollback policies are intentionally off by default.

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

## Updating Hostlet

Hostlet checks GitHub Releases for stable updates. The Settings page shows the current version, latest checked version, release notes, and the command to run.

Check from the server:

```bash
hostlet update check
hostlet update --dry-run
```

Apply an update:

```bash
hostlet update
```

The update command verifies release assets and checksums, creates a pre-update backup by default, saves the previous CLI binary and current Compose files, downloads the new CLI, restarts the Compose stack, and runs `hostlet doctor`. If the CLI replacement fails because the binary is installed under `/usr/local/bin`, rerun the command with appropriate privileges.

Releases may include `hostlet-release.json`. When present, Hostlet uses it to show the exact version, minimum supported direct-upgrade version, release date, and whether Compose or database migrations are expected. If the manifest is not present, Hostlet falls back to GitHub release metadata and the checksum asset.

Rollback support restores the previous CLI binary, restores saved Compose files when available, and restarts services:

```bash
hostlet update rollback
```

Database rollback is not automatic. Keep the pre-update backup until the upgraded stack has been validated.

## Public Tunnel Exposure

Public exposure is optional and per app.

Required `.env` values:

```bash
HOSTLET_BASE_DOMAIN=example.com
# Optional legacy cleanup prefix for old managed app records.
HOSTLET_DOMAIN_PREFIX=hostlet-
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ZONE_ID=...
CLOUDFLARE_TUNNEL_TARGET=<tunnel-id>.cfargotunnel.com
CLOUDFLARE_TUNNEL_TOKEN=...
```

Behavior:

- New apps default to private.
- When `HOSTLET_BASE_DOMAIN` is configured, blank app domains default to `<app-slug>.<HOSTLET_BASE_DOMAIN>`, for example `runcomp.shanekanterman.dev`.
- **Publish URL** creates or updates a proxied Cloudflare CNAME/Tunnel record for the app hostname.
- **Make private** deletes the app hostname record.
- Hostlet only manages single-label app hostnames under `HOSTLET_BASE_DOMAIN`.
- Reserved labels such as `www`, `mail`, `api`, and `hostlet` are blocked.
- Hostlet stores each created Cloudflare record in `app_public_dns_records` and will not claim unrelated CNAME records.
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
PUBLIC_WEBHOOK_URL/webhooks/github
```

If `PUBLIC_WEBHOOK_URL` is empty, Hostlet falls back to `PUBLIC_API_URL` for the webhook setup UI. GitHub webhooks require the payload URL to be public HTTPS. These do not work for real GitHub webhook delivery:

```text
http://localhost:8080
http://10.0.0.194:8080
http://192.168.1.20:8080
```

Those URLs are fine for Device Flow sign-in and LAN/manual deploys.

If you keep `PUBLIC_API_URL` in LAN mode but expose webhooks through a tunnel, set:

```text
PUBLIC_WEBHOOK_URL=https://hostlet.example.com
```

In LAN mode, push to GitHub and click **Deploy latest** in Hostlet.

For auto-redeploy, run tunnel mode or another public HTTPS reverse proxy:

```bash
hostlet up --tunnel
```

When you enable **Auto redeploy on branch push**, Hostlet uses the connected GitHub token to create or update the repository webhook:

- Payload URL: `PUBLIC_WEBHOOK_URL/webhooks/github`, or `PUBLIC_API_URL/webhooks/github` when `PUBLIC_API_URL` is public HTTPS
- Content type: `application/json`
- Secret: `GITHUB_WEBHOOK_SECRET`
- Events: push

When a push event matches an app repository and branch, Hostlet creates a deployment for that exact commit SHA only if **Auto redeploy on branch push** is enabled for that app. Webhook deliveries are deduplicated by GitHub delivery ID and the app detail page shows the latest webhook result.

Manual webhook setup is still possible if you do not want Hostlet to manage the hook. Use the same payload URL, content type, secret, and push event configuration shown above.

## Deployment Target

Hostlet 0.2.0 is intentionally local-machine-only. The UI, API, database, Caddy, and local agent run on the same host, and apps deploy as Docker containers on that host.

Remote VPS agent registration and install commands are disabled for this release while the local deploy path is hardened.

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

## App Data Persistence

Every deployed app receives a writable persistent Docker volume mounted at `/data`. Hostlet names it `hostlet-app-data-<app-id>`, so redeploys and updates replace containers without deleting app data.

The agent injects:

```bash
HOSTLET_DATA_DIR=/data
DATA_DIR=/data
```

If an app explicitly sets `DATA_DIR`, Hostlet preserves that value and still sets `HOSTLET_DATA_DIR`. Deleting an app removes its persistent data directory.

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

`hostlet init` writes `DOCKER_GID` from `/var/run/docker.sock` so the non-root local agent can talk to Docker. If Docker socket permissions change, regenerate `.env` or set:

```bash
DOCKER_GID=$(stat -c '%g' /var/run/docker.sock)
```

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
