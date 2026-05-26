# Hostlet 0.4.0 Cloud Branch Update

Date: 2026-05-26
Branch: `hostlet-0.4.0-cloud-foundation`

This update completes the 0.4.0 final polish work for the private Hostlet Cloud beta and keeps the self-hosted path intact.

## Updated

- Hardened cloud tenancy with shared cloud request context, session revocation, customer-scoped authorization, compute gates, and cross-user job isolation.
- Tightened GitHub App and Stripe flows with install-state validation, installation ownership checks, webhook-authoritative subscription state, duplicate webhook handling, and safer browser-facing errors.
- Improved the managed agent runtime with job lease renewal, loopback-only published ports, safer Docker Compose validation, atomic Caddy route updates, route restoration on reload failure, and retry/backoff for API status posts.
- Updated cloud/self-host UI behavior across dashboard, app creation, app detail, deployment logs, settings, login, navigation, and mobile layouts.
- Expanded release documentation, validation checklists, threat model, ownership review, and security guidance for Hostlet Cloud 0.4.0.
- Added CI smoke coverage for cloud API mode, web routes, production Compose validation, and responsive QA.
- Ignored stale `dist/` release artifacts and kept local secret files ignored.

## Validation

The branch was validated locally with Rust formatting/checks/tests, web lint/build, cloud API smoke, web route smoke, responsive QA, Compose config checks, `git diff --check`, and a redacted gitleaks scan before pushing.

## Secret Handling

Local secret files may be inspected on Shane's machine for validation and deployment, but raw secret values must never be printed, committed, pushed, copied into docs, or exposed in release artifacts.
