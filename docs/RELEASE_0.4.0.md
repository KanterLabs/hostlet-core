# Hostlet 0.4.0 Release Notes

Hostlet 0.4.0 starts the hybrid cloud track while keeping the existing self-hosted product path intact.

## Changes

- Adds `HOSTLET_MODE=self_hosted|cloud` as the explicit runtime mode boundary.
- Adds cloud integration configuration plumbing for GitHub App, Stripe, and Hostlet Cloud status checks.
- Adds cloud account, GitHub installation, Stripe subscription, entitlement, usage, and webhook event tables.
- Adds a direct-origin Caddy config for `hostlet.cloud` and `*.apps.hostlet.cloud`.
- Updates production Compose to pass cloud mode, GitHub App, and Stripe environment values.
- Documents the single-VM Hostlet Cloud MVP plan.

## Notes

The 0.4.0 foundation does not yet turn Hostlet into a complete paid SaaS. The hosted account flow, billing checkout, GitHub App install handling, and cloud app lifecycle APIs are the next implementation layer on top of this release foundation.
