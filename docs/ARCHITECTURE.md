# Architecture

Hostlet is a self-hosted deployment control plane with a browser UI, Rust API, deployment agent, PostgreSQL database, local Caddy router, and optional Cloudflare Tunnel.

## System Layout

```text
Browser
  |
  | HTTP :3000
  v
Next.js web UI
  |
  | HTTP API / WebSocket logs :8080
  v
Rust API control plane
  |
  | PostgreSQL
  v
Postgres

Rust API <---- authenticated WebSocket/events ----> Hostlet agent
                                                     |
                                                     | Docker CLI/socket
                                                     v
                                                App containers

Cloudflare edge -> cloudflared -> 127.0.0.1:18080 -> Caddy -> local app container port
```

## Services

### Web

`apps/web` is a Next.js dashboard. It provides:

- first-run password and unlock flow
- GitHub connection status
- local machine status
- app list and app creation
- app detail page with deploy, rollback, public tunnel toggle, delete, runtime health, manual container restart, and resource usage
- app settings and encrypted environment-variable editing
- deployment detail page with status and live logs
- logs index
- settings page with GitHub, Cloudflare, and Hostlet update status plus GitHub reconnect

The web app calls the API with cookies and sends `X-Hostlet-CSRF: 1` on state-changing requests.

### API

`apps/api` is an Axum application. It owns:

- database migrations
- setup password and unlock cookies
- GitHub OAuth
- GitHub repository listing
- GitHub webhooks
- app settings and encrypted environment variable management
- app/server CRUD
- deployment and rollback job creation
- authenticated agent WebSocket
- authenticated agent event ingestion
- deployment log storage and WebSocket fanout
- runtime health snapshot/event storage
- Hostlet version and release update checks
- Cloudflare DNS management for app tunnel open/close
- basic in-memory rate limiting for high-risk public endpoints

The API does not require Docker access. Docker operations are delegated to agents.

### Agent

`apps/agent` connects to the API over an authenticated WebSocket. It:

- verifies signed deploy and rollback jobs
- clones or updates Git repositories
- builds Docker images
- generates Dockerfiles for supported Node apps when no Dockerfile exists
- starts containers with reduced privileges and optional resource limits
- health-checks new containers
- continuously checks current app runtime health
- restarts the current app container when the owner requests manual recovery
- writes Caddy routes
- reports status and logs to the API
- publishes Docker resource and runtime health snapshots for local apps

Hostlet currently runs one local agent on the same machine as the UI/API. The local agent uses host networking and Docker socket access so it can build images, start app containers, probe runtime health, restart app containers on request, and reload the local Caddy router. Remote VPS agents and install commands remain deferred.

### Caddy and Cloudflare Tunnel

Local Compose includes `hostlet-caddy`, bound to loopback on port `18080`. Cloudflare Tunnel forwards wildcard hostname traffic to that Caddy listener.

The Caddyfile imports per-app snippets from `/var/lib/hostlet/caddy/*.caddy`. Each snippet matches an app hostname and reverse-proxies to that app container's assigned loopback port.

Public exposure is controlled by DNS:

- tunnel open: create/update a proxied CNAME/Tunnel record
- tunnel close: delete that app record

The wildcard cloudflared ingress can stay running even when no app is public.

## Database

Main tables:

- `users`: GitHub-backed users
- `github_accounts`: encrypted GitHub access tokens
- `servers`: deploy targets; Hostlet currently seeds and uses only the local machine
- `apps`: deployable app configuration
- `app_env_vars`: encrypted app environment variables
- `deployments`: deployment records and route metadata
- `deployment_logs`: stored build/runtime logs
- `rollback_events`: rollback audit records
- `webhook_events`: GitHub webhook dedupe and payload storage
- `webhook_app_events`: per-app webhook outcomes for app detail status
- `agent_jobs`: local cleanup jobs such as app deletion
- `settings`: control-plane password hash and small settings
- `app_resource_snapshots`: latest agent-reported Docker stats
- `app_health_snapshots`: latest runtime health per app/current container
- `app_health_events`: recent runtime health history
- `app_public_dns_records`: app-owned Cloudflare DNS records

## Deployment Flow

1. User clicks **Deploy** or a GitHub push webhook matches an app.
2. API inserts a deployment row.
3. API decrypts app env vars, builds a job payload, signs it, and sends it to the app server's connected agent.
4. Agent verifies the signature.
5. Agent fetches the repo and checks out either `HEAD` or a webhook commit SHA.
6. Agent builds a Docker image from the repo Dockerfile or a generated Node Dockerfile.
7. Agent starts a new container on a host loopback port with the app's persistent data directory mounted at `/data`.
8. Agent health-checks the configured path.
9. If healthy, agent writes/updates route config and reloads Caddy.
10. Agent reports success with image, container, local URL, and published port.
11. API marks the app's current deployment.

Failed health checks preserve the previous working app. Failed new containers are left available for inspection.

Each local app gets a stable Docker volume named `hostlet-app-data-<app-id>`, mounted into every deployment as `/data`. The agent injects `HOSTLET_DATA_DIR=/data` and, when the app has not set it explicitly, `DATA_DIR=/data`. Redeploys and rollbacks reuse the same volume; deleting the app removes it.

## Runtime Health Flow

Runtime health is intentionally separate from deployment status.

1. API exposes the current app health targets to the authenticated local agent.
2. Agent checks each current container every 60 seconds using the configured health path and published loopback port.
3. HTTP `2xx` and `3xx` responses are treated as healthy.
4. One failure marks an app `degraded`; three consecutive failures mark it `unhealthy`.
5. Agent sends `health_status` events to the API.
6. API updates `app_health_snapshots` and appends `app_health_events`.
7. App list, dashboard, and app detail poll these health fields.
8. The owner can request **Check now** or **Restart container** from the app detail page.

## Rollback Flow

1. User clicks **Rollback**.
2. API finds the previous successful deployment with route metadata.
3. API creates a rollback deployment row and rollback event.
4. Agent updates Caddy routing to the previous container port.
5. API marks the rollback deployment as `rolled_back`.

Rollback changes routing only; it does not delete containers or images.

## Public Exposure Flow

1. User clicks **Publish URL** on an app.
2. API verifies the hostname is a Hostlet-managed hostname:
   - under `HOSTLET_BASE_DOMAIN`
   - single label before the base domain
   - not one of Hostlet's reserved labels such as `www`, `mail`, `api`, or `hostlet`
   - either owned by that app in `app_public_dns_records`, unclaimed in Cloudflare, or an old `HOSTLET_DOMAIN_PREFIX` legacy record
3. API creates or updates a proxied CNAME/Tunnel record pointing at `CLOUDFLARE_TUNNEL_TARGET` and records ownership in `app_public_dns_records`.
4. API marks `apps.public_exposure=true`.

Making the app private deletes only that app's owned DNS record and marks `public_exposure=false`.

## Hostlet Update Flow

Update checks use public GitHub Releases and do not require a GitHub user token.

1. The API checks the latest release on startup if cached update data is older than 24 hours and `HOSTLET_UPDATE_CHECKS` is not disabled.
2. The Settings page displays the installed version, latest checked version, release notes, minimum supported direct-upgrade version, and migration flags.
3. The CLI provides `hostlet status`, `hostlet update check`, `hostlet update --dry-run`, `hostlet update`, and `hostlet update rollback`.
4. Release metadata prefers `hostlet-release.json` when present, with fallback to GitHub release metadata and the `.sha256` asset.
5. `hostlet update` verifies the release asset checksum, creates a pre-update backup by default, saves the previous CLI binary and current Compose files, replaces the CLI, restarts the Compose stack, and runs `hostlet doctor`.
6. Rollback restores the previous CLI binary, restores saved Compose files when available, and restarts services. Database rollback remains manual through the pre-update backup.

`hostlet status` and `hostlet doctor` use the local agent token from `.env` to call `/api/system/operator-status`, which reports aggregate app health and server status without requiring a browser session cookie.

## Trust Boundaries

- Browser to web/API: cookie-based control-plane session and unlock cookie.
- API to GitHub: OAuth access token encrypted at rest.
- API to agent: agent token plus signed job payloads.
- Agent to Docker: privileged local boundary; the agent must be treated as host-trusted.
- Cloudflare to local router: tunnel ingress forwards to loopback-only Caddy.

## Current Constraints

- One local default server is seeded by environment.
- Remote VPS agents are intentionally disabled.
- No queue worker exists; jobs are sent directly to connected agents.
- Deployment logs and resource snapshots are retained indefinitely unless cleaned manually. Runtime health snapshots keep the latest state, and runtime health events are pruned to seven days or the latest 500 events per app.
- Automatic self-healing policies remain disabled; 0.2.0 provides manual check/restart, redeploy, and rollback actions.
