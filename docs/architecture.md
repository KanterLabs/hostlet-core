# Architecture

Hostlet is a deployment control plane with two runtime modes.

```text
HOSTLET_MODE=self_hosted
HOSTLET_MODE=cloud
```

## Components

- Web UI: Next.js dashboard.
- API: Rust Axum control plane.
- Postgres: persistent control-plane state.
- Agent: Rust deployment worker.
- Caddy: app/control-plane router.
- cloudflared: optional Cloudflare Tunnel connector.

## Self-Hosted Layout

```text
Browser -> Web UI -> API -> Postgres
                     |
                     v
                  Agent -> Docker -> App containers
                     |
                     v
                  Caddy routes
```

Self-hosted Hostlet runs one local agent on the same machine as the UI/API. The agent uses Docker socket access and host routing privileges to build apps, start containers, probe health, update Caddy, and report status.

## Hostlet Cloud Layout

```text
Cloudflare edge -> Hostlet Cloud ingress -> Caddy -> Web/API
                                                |
                                                v
                                          Managed app routes
```

Hostlet Cloud runs the hosted web/API, database, Caddy, managed agent, and customer app containers on Hostlet-operated infrastructure. The managed agent is privileged infrastructure; customer apps are untrusted.

## Data Model

Core state includes:

- users and sessions
- GitHub account/install state
- app configuration
- encrypted app environment variables
- deployments and deployment logs
- agent jobs and runtime health
- Cloudflare-owned app DNS records
- cloud user, subscription, entitlement, usage, and webhook state

## Deployment Flow

1. User creates or deploys an app.
2. API inserts deployment/job state.
3. API signs a job for the agent.
4. Agent verifies the signature.
5. Agent fetches the repo and builds the app.
6. Agent starts a new container with loopback routing.
7. Agent health-checks the container.
8. Agent updates Caddy routing on success.
9. API records final deployment state and logs.

Failed deployments preserve the previous working app route.

## Runtime Boundaries

- Browser to API: cookie session, unlock state, CSRF/origin checks.
- API to agent: authenticated and signed jobs.
- Agent to Docker/Caddy: privileged host operations.
- App containers: untrusted customer code.
- API to providers: GitHub, Stripe, and Cloudflare calls over HTTPS.

Hostlet Cloud adds cloud session, GitHub App, billing, and tenant isolation checks before managed compute actions.
