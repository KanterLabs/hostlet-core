# Hostlet Cloud

Hostlet Cloud is the managed SaaS app-hosting service at `https://hostlet.cloud`.

## Product Boundary

Hostlet Cloud runs in `HOSTLET_MODE=cloud` on Hostlet-operated infrastructure.

- Control-plane host: `hostlet.cloud`.
- Managed app hosts: `*.hostlet.cloud`.
- Repository access: GitHub OAuth plus GitHub App installation.
- Billing: Stripe-backed subscription state.
- Compute: managed Hostlet worker agent.

Self-hosted installs do not need a Hostlet Cloud account and do not depend on Stripe or the Hostlet GitHub App.

## Cloud App Flow

1. User signs in at `hostlet.cloud`.
2. User installs or authorizes the Hostlet GitHub App.
3. User completes billing.
4. Hostlet Cloud verifies active/trialing subscription state.
5. User creates and deploys a supported app.
6. The managed worker builds/runs the app and routes it under `*.hostlet.cloud`.

Checkout completion alone is not authoritative. Provider webhooks must update subscription state before compute is available.

## SaaS Security Rules

Hostlet Cloud customer apps are untrusted code.

Customer apps must never receive:

- worker tokens
- Cloudflare tokens
- Stripe secrets
- GitHub App private keys
- direct database access
- direct job-queue access
- platform control-plane credentials

Provider credentials and worker credentials stay in Hostlet-operated infrastructure. Public docs must not include exact production inventory or secret values.

## Current Cloud Limits

- Private beta.
- Single-service Dockerfile or generated Node apps.
- No cloud Compose apps.
- No custom domains.
- No managed databases.
- No customer-selected workers.
- No arbitrary CPU/RAM edits.
- No customer-controlled public/private toggles.
- Live Stripe mode is deferred until explicitly enabled.

## Operations

Hostlet Cloud production runs tagged release images from GHCR. See [Hostlet Cloud Infrastructure](hostlet-cloud-infrastructure.md) for the public-safe runbook. Exact production inventory belongs only in the ignored private tracker.
