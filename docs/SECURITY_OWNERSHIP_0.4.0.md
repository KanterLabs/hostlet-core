# Hostlet 0.4.0 Sensitive-Code Ownership Review

## Scope

This review covers security-sensitive Hostlet 0.4.0 paths:

- `apps/api/src/auth.rs`
- `apps/api/src/crypto.rs`
- `apps/api/src/github_app.rs`
- `apps/api/src/web.rs`
- `apps/api/src/deploy.rs`
- `apps/api/src/agent.rs`
- `apps/api/migrations/021_cloud_accounts.sql`
- `infra/docker-compose.yml`
- `infra/docker-compose.prod.yml`

The security ownership-map skill was attempted with its graph script, but the local Python environment was missing `networkx` and could not create a venv because `python3-venv` is unavailable. As a fallback, this review uses bounded `git log --since='24 months ago'` history for the scoped paths.

## Findings

All scoped sensitive files are owned by one git author in the last 24 months:

| Path | Primary author | Touch count |
| --- | --- | ---: |
| `apps/api/src/web.rs` | DCT Git Specialist `<kanterman04@gmail.com>` | 12 |
| `apps/api/src/deploy.rs` | DCT Git Specialist `<kanterman04@gmail.com>` | 9 |
| `apps/api/src/auth.rs` | DCT Git Specialist `<kanterman04@gmail.com>` | 8 |
| `infra/docker-compose.yml` | DCT Git Specialist `<kanterman04@gmail.com>` | 9 |
| `infra/docker-compose.prod.yml` | DCT Git Specialist `<kanterman04@gmail.com>` | 8 |
| `apps/api/src/agent.rs` | DCT Git Specialist `<kanterman04@gmail.com>` | 5 |
| `apps/api/src/crypto.rs` | DCT Git Specialist `<kanterman04@gmail.com>` | 2 |
| `apps/api/src/github_app.rs` | DCT Git Specialist `<kanterman04@gmail.com>` | 1 |
| `apps/api/migrations/021_cloud_accounts.sql` | DCT Git Specialist `<kanterman04@gmail.com>` | 1 |

## Risk Interpretation

- Bus factor is 1 for all sensitive areas. This is expected for Shane's personal project, but it increases the chance that auth, billing, agent, and infra assumptions are not independently challenged before release.
- The most review-sensitive low-touch files are `apps/api/src/github_app.rs` and `apps/api/migrations/021_cloud_accounts.sql`, because they define GitHub App trust and cloud tenancy/billing state.
- The highest-churn sensitive file is `apps/api/src/web.rs`; it should get focused pre-tag review because it contains billing webhooks, app CRUD, env vars, job controls, and entitlement checks.

## Required Pre-Tag Review

Before tagging `v0.4.0`, do a focused review of:

- Cloud auth/session binding in `auth.rs`.
- Stripe webhook state transitions and plan gates in `web.rs`.
- GitHub App installation ownership and webhook validation in `auth.rs`, `github_app.rs`, and `github.rs`.
- Agent job authentication, signing, lease renewal, and event ingestion in `agent.rs` and `deploy.rs`.
- Docker/Caddy command execution, route rollback, Compose validation, and loopback port binding in `apps/agent/src/main.rs`.
- Cloud account migrations and unique constraints in `021_cloud_accounts.sql`.
- Production Compose exposure and Docker socket access in `infra/docker-compose.prod.yml`.

## Residual Risk

Single-maintainer ownership is acceptable for this private beta only if the above pre-tag review and validation gates are completed. If Hostlet Cloud expands beyond a private beta, add an explicit reviewer or CODEOWNERS-equivalent review requirement for auth, billing, agent, crypto, migrations, and infra changes.
