# Architecture

Hostlet is a deployment control plane with two 0.4.0 modes:

- `HOSTLET_MODE=self_hosted`: single-machine self-hosted control plane with browser UI, Rust API, local deployment agent, PostgreSQL database, Caddy router, and optional Cloudflare Tunnel.
- `HOSTLET_MODE=cloud`: private hosted beta at `hostlet.cloud` with hosted web/API, managed worker agent, Caddy direct-origin routing for `hostlet.cloud` and `*.hostlet.cloud`, GitHub App repository access, and Stripe sandbox billing gates.

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

Self-hosted public path:
Cloudflare edge -> cloudflared -> 127.0.0.1:18080 -> Caddy -> local app loopback port

Cloud beta public path:
Cloudflare edge -> hostlet VM Caddy -> web/API or managed app loopback port
```

## Services

### Web

`apps/web` is a Next.js dashboard. It provides:

- first-run password and unlock flow
- GitHub connection status
- machine or managed worker status, depending mode
- app list and app creation
- app detail page with deploy, rollback where supported, delete, runtime health, manual container restart, resource usage, and mode-specific settings
- app settings and encrypted environment-variable editing
- deployment detail page with status and live logs
- logs index
- settings page with mode-specific GitHub, Cloudflare, Hostlet update, billing, and job status panels

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
- cloud account/session, GitHub App installation, Stripe subscription, entitlement, and webhook state
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

Self-hosted Hostlet currently runs one local agent on the same machine as the UI/API. The local agent uses host networking and Docker socket access so it can build images, start app containers, probe runtime health, restart app containers on request, and reload the local Caddy router. Remote self-hosted VPS agents and install commands remain deferred.

In cloud mode, the agent is a managed Hostlet worker. It is still host-privileged because it controls Docker and Caddy, but customer apps receive only their configured app environment and must never receive worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, direct database access, or direct job-queue access.

### Caddy, Direct Origin, and Cloudflare Tunnel

Local Compose includes `hostlet-caddy`, bound to loopback on port `18080`. Cloudflare Tunnel forwards wildcard hostname traffic to that Caddy listener.

The Caddyfile imports per-app snippets from `/var/lib/hostlet/caddy/*.caddy`. Each snippet matches an app hostname and reverse-proxies to that app container's assigned loopback port.

Public exposure is controlled by DNS:

- tunnel open: create/update a proxied CNAME/Tunnel record
- tunnel close: delete that app record

The wildcard cloudflared ingress can stay running even when no app is public.

Cloud mode uses direct-origin Caddy config for `hostlet.cloud` and `*.hostlet.cloud`. The control-plane host routes to web/API services and app hostnames route to managed app snippets. App snippets are written through temp files and rename; failed Caddy reloads restore the previous route state.

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
- `cloud_users`, `cloud_sessions`, `cloud_github_installations`, `cloud_subscriptions`, `cloud_plan_entitlements`, `cloud_usage_buckets`, and `cloud_webhook_events`: cloud identity, tenancy, billing, entitlement, and provider webhook dedupe state

## Deployment Flow

1. User clicks **Deploy** or a GitHub push webhook matches an app.
2. API inserts a deployment row.
3. API decrypts app env vars, builds a job payload, signs it, and sends it to the app server's connected agent.
4. Agent verifies the signature.
5. Agent fetches the repo and checks out either `HEAD` or a webhook commit SHA.
6. Agent builds a Docker image from the repo Dockerfile or a generated Node Dockerfile.
7. Agent starts a new container on a host loopback port with the app's persistent data directory mounted at `/data` for single-service apps.
8. Agent health-checks the configured path.
9. If healthy, agent writes/updates route config and reloads Caddy.
10. Agent reports success with image, container, local URL, and published port.
11. API marks the app's current deployment.

Failed health checks preserve the previous working app. Failed new containers are left available for inspection.

Each single-service app gets a stable Docker volume named `hostlet-app-data-<app-id>`, mounted into every deployment as `/data`. The agent injects `HOSTLET_DATA_DIR=/data` and, when the app has not set it explicitly, `DATA_DIR=/data`. Redeploys and rollbacks reuse the same volume; deleting the app removes it. Compose apps keep their declared named volumes; Hostlet does not inject `/data` into arbitrary Compose services.

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

Rollback changes routing only; it does not delete containers or images. Compose rollback is disabled for 0.4.0 and returns a clear unsupported response.

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
- API to GitHub App: cloud installation token generation and installation ownership checks.
- API to Stripe: sandbox checkout, portal, webhook signature validation, subscription state, and entitlement gates.
- API to agent: agent token plus signed job payloads.
- Agent to Docker: privileged local boundary; the agent must be treated as host-trusted.
- Cloudflare to local router: tunnel ingress forwards to loopback-only Caddy.

## Current Constraints

- Self-hosted mode seeds and uses one local default server.
- Remote self-hosted VPS agents are intentionally disabled.
- Hostlet Cloud 0.4.0 is single-worker/single-VM beta; multi-worker scheduling is deferred.
- Cloud custom domains, Compose, managed databases, persistent disk upsells, arbitrary resource edits, and Stripe live mode are deferred.
- No queue worker exists; jobs are sent directly to connected agents.
- Deployment logs and resource snapshots are retained indefinitely unless cleaned manually. Runtime health snapshots keep the latest state, and runtime health events are pruned to seven days or the latest 500 events per app.
- Automatic self-healing policies remain disabled; Hostlet provides manual check/restart, redeploy, and rollback where supported.
