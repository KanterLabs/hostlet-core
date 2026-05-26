# Hostlet 0.4.0 Release Notes

Hostlet 0.4.0 introduces the private Hostlet Cloud beta while preserving the self-hosted single-machine product path.

## Highlights

- Adds `HOSTLET_MODE=self_hosted|cloud` as the explicit runtime boundary.
- Adds hosted `hostlet.cloud` and `*.hostlet.cloud` routing support through direct-origin Caddy config.
- Adds cloud account, cloud session, GitHub installation, Stripe subscription, entitlement, usage, and provider webhook dedupe tables.
- Adds GitHub App installation state validation and ownership/admin checks for cloud repository access.
- Adds Stripe sandbox checkout, customer portal, subscription webhook handling, signature timestamp tolerance, and subscription-state compute gates.
- Adds shared cloud request context so cloud authorization resolves both legacy user state and cloud user state consistently.
- Gates cloud app create, env changes, deploy, restart, rollback, job retry/cancel, and runtime mutation behind GitHub App installation and active/trialing subscription state.
- Rejects unsupported cloud settings: Compose, custom domains, public/private toggles, auto-deploy toggles, and arbitrary CPU/RAM edits.
- Keeps self-hosted GitHub Device Flow, local deploys, Cloudflare Tunnel, webhooks, publish/private controls, single-service rollback, restart, delete, backup, and update workflows.
- Binds raw Docker app ports to loopback and routes app traffic through Caddy.
- Restores previous Caddy route state if a route reload fails.
- Adds agent job lease renewal for long builds.
- Disables Compose rollback for 0.4.0 with clear API/UI messaging.
- Expands security docs with a 0.4.0 threat model, sensitive-code ownership review, and release validation gates.

## Hostlet Cloud Beta Limits

Hostlet Cloud 0.4.0 is a private beta, not a general production SaaS.

- Stripe remains sandbox-only.
- Cloud MVP supports single-service Dockerfile/generated Node apps.
- Cloud custom domains are deferred.
- Cloud Compose support is deferred.
- Managed databases are deferred.
- Persistent disk upsells are deferred.
- Multi-worker scheduling is deferred.
- Production Stripe live mode is deferred.
- Customer-controlled public/private toggles, auto-deploy toggles, and arbitrary CPU/RAM edits are deferred.

## Self-Hosted Notes

Self-hosted Hostlet remains a single-machine deployment system for this release. Remote self-hosted VPS agents are still disabled while the local agent, Caddy routing, update, backup, and validation paths are hardened.

Single-service apps receive a Hostlet-managed persistent Docker volume mounted at `/data`. Compose apps keep their declared named volumes; Hostlet does not inject `/data` into arbitrary Compose services.

## Validation Required Before Tagging

Before tagging `v0.4.0`, complete the gates in [VALIDATION_0.4.0.md](VALIDATION_0.4.0.md), including:

- automated Rust/web checks
- self-hosted regression
- cloud-local tenancy and billing checks
- `hostlet.cloud` infrastructure validation
- sandbox paid deploy
- upgrade/rollback
- backup/restore
- release artifact inspection
- security gate
- responsive QA
