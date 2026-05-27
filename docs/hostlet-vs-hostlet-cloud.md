# Hostlet vs Hostlet Cloud

Hostlet and Hostlet Cloud use the same codebase and release tags, but they are different products at runtime.

## Hostlet

Hostlet is the open-source self-hosted product.

- Runs on your server.
- Uses your Docker daemon.
- Uses GitHub OAuth Device Flow for repository access.
- Can expose apps through LAN, Cloudflare Tunnel, or another trusted reverse proxy.
- Does not require a Hostlet Cloud account, Stripe subscription, or GitHub App installation.

Self-hosted Hostlet is currently a single-machine system. Remote self-hosted VPS agents are deferred.

## Hostlet Cloud

Hostlet Cloud is the managed SaaS hosting service at `hostlet.cloud`.

- Runs on Hostlet-operated infrastructure.
- Deploys apps to managed Hostlet workers.
- Uses `*.hostlet.cloud` app URLs.
- Uses GitHub OAuth plus GitHub App installation for repository access.
- Gates managed compute behind billing and subscription state.
- Keeps provider credentials and platform worker secrets inside Hostlet-operated infrastructure.

## Shared Release Model

Both products ship from the same branch and tags:

- one `main` branch
- one `vX.Y.Z` release tag
- one GHCR image set for API, web, and agent

The runtime mode controls behavior:

```text
HOSTLET_MODE=self_hosted
HOSTLET_MODE=cloud
```

Cloud-only differences are runtime gated: hosted compute, billing, GitHub App install flow, cloud-specific UI labels, and SaaS operations.

## Current Limits

Self-hosted limits:

- single-machine deployment target
- no remote self-hosted VPS agents
- no built-in managed database provisioning
- Compose rollback disabled for the current release

Hostlet Cloud limits:

- private beta
- Stripe sandbox billing for the current release
- single-service Dockerfile or generated Node apps
- no cloud Compose apps
- no custom domains
- no managed databases
- no customer-selected workers
- no arbitrary customer CPU/RAM edits
- no live Stripe mode until explicitly enabled
