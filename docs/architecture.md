# Architecture

Hostlet is a self-hosted deployment control plane.

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

## Data Model

Core state includes:

- users and sessions
- GitHub account state
- app configuration
- encrypted app environment variables
- deployments and deployment logs
- agent jobs and runtime health
- Cloudflare-owned app DNS records

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
- API to providers: GitHub and Cloudflare calls over HTTPS.
