# Hostlet 0.4.0 Final Polish Plan

## Summary

Hostlet 0.4.0 should be treated as a private hosted cloud beta that is safe enough for real first users, with Stripe still in sandbox. The release is not just UI polish: it must close cloud tenancy, billing, GitHub App, runtime reliability, docs, and validation gaps while preserving self-hosted behavior.

The long-running implementation agent should work this file top to bottom, keep checklist status current, and only mark an item complete after implementation and verification.

## Secret Handling For Implementation

- Shane has explicitly authorized the implementation agent to inspect local secret values when needed to finish this plan. The agent should not stop to ask for keys if the values are already available on this machine.
- The implementation agent may inspect local secret files such as `.env`, `.env.prod`, and `infra/.env` to verify required variables, run local/cloud validation, and diagnose configuration issues.
- Do not print, paste, commit, copy into docs, or expose actual secret values. Summaries should say only whether each value is present, empty, placeholder-looking, or missing.
- `.env.prod` is the complete source for the `hostlet.cloud` validation path. `.env` and `infra/.env` are local/self-host oriented and are not expected to contain all GitHub App or Stripe cloud values.
- Before any commit or push, verify secret files remain ignored/untracked and that docs, logs, release artifacts, and browser bundles do not contain raw secret values.

## Release Bar

- A real user can sign into `hostlet.cloud`, install the GitHub App, complete Stripe sandbox checkout, create one supported app, deploy it, view logs, restart it, rollback where supported, and delete it.
- An unpaid or GitHub-uninstalled cloud user cannot create compute or mutate existing app runtime state.
- Two cloud users cannot see or control each other's apps, jobs, logs, env vars, or billing state.
- Security review for cloud tenancy, billing, GitHub App install, API/agent trust, frontend browser posture, and privileged runtime paths is documented before tagging.
- Self-hosted setup, unlock, Device Flow login, local deploy, publish/private URL, rollback, restart, and delete still work.
- Docs match the actual cloud/self-host product.
- CI and manual validation cover the full release bar before tagging `v0.4.0`.

## Phase 1: Cloud Safety And Tenancy

- [x] Add a shared cloud request context that resolves both legacy `users.id` and `cloud_users.id`.
- [x] Use the shared cloud request context for every customer-scoped route so authorization is not reimplemented per handler.
- [x] Gate cloud app create, update, env changes, deploy, restart, rollback, health-check jobs, job retry/cancel, and cleanup behind cloud session, GitHub App install, and active Stripe subscription.
- [x] Block cloud access to operator-only cleanup and other self-hosted maintenance endpoints unless they are explicitly designed for cloud customers.
- [x] Fix multi-user job isolation so one cloud user cannot list, inspect, retry, cancel, or clean up another user's jobs because the worker server is `kind='local'`.
- [x] Server-side reject unsupported cloud settings: Compose, custom domain edits, public/private toggles, auto-deploy toggles, and arbitrary CPU/RAM edits.
- [x] Enforce minimum plan basics from `cloud_plan_entitlements`, especially active app count and starter resource limits.
- [x] Revoke `cloud_sessions.revoked_at` on logout, not only browser cookies.
- [x] Add tests for unpaid users, missing GitHub App install, inactive subscriptions, and cross-user isolation.

## Phase 2: Billing And GitHub App Hardening

- [x] Add state validation to GitHub App install flow.
- [x] Prevent accidental GitHub installation reassignment unless ownership/admin access is verified.
- [x] Tighten Stripe subscription activation so checkout alone cannot leave indefinite active billing without real subscription state.
- [x] Stop returning raw Stripe or GitHub upstream error strings to browser users; log detailed provider errors server-side and return actionable safe messages.
- [x] Add Stripe webhook tests for duplicate events, missing metadata, checkout completion, subscription updates, cancellation, and deletion.
- [x] Verify webhook handlers never trust browser CSRF/origin exemptions as authentication; Stripe and GitHub webhooks must rely on provider signatures and dedupe only.
- [x] Keep Stripe sandbox for 0.4.0, but make the code path production-shaped.

## Phase 3: Runtime And Infra Reliability

- [x] Make Caddy route updates atomic: failed reload must not leave snippets that later activate a failed deployment.
- [x] Decide and enforce private/public bind behavior; raw ephemeral app ports should not be reachable publicly unless explicitly intended by firewall policy.
- [x] Treat the cloud agent/Docker/Caddy boundary as privileged host control in docs and validation; customer apps must never receive worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, or direct job-queue access.
- [x] Add agent job lease renewal or long-build protection because current leases are shorter than build timeouts.
- [x] Improve status reconciliation when agent event posts fail after Docker or Caddy side effects.
- [x] Fix Compose validation for long-syntax bind mounts and unsupported network/port patterns.
- [x] Either make Compose rollback real or disable/label Compose rollback clearly for 0.4.0.
- [x] Fix or document Compose `/data` behavior so it matches the product claim.
- [x] Stop dev compose `cloudflared` from crash-looping without an explicit tunnel profile/token.

## Phase 4: Cloud/Self-Hosted UX Polish

- [x] Audit `/`, `/apps`, `/apps/new`, `/apps/[id]`, `/deployments/[id]`, `/logs`, `/settings`, and `/login` for mode-specific copy and controls.
- [x] Cloud should say worker/Hostlet Cloud, not machine/local/tunnel/webhook/update where irrelevant.
- [x] Self-hosted mode should keep Device Flow, machine, Cloudflare Tunnel, webhook, and local deploy language.
- [x] Replace clickable `aria-disabled` setup links with real disabled states or setup CTAs.
- [x] Add clear disabled reasons on create app for missing billing, missing GitHub App, missing env vars, missing repo, and non-deployable repo inspection.
- [x] Keep users on deployment logs after success with clear CTAs instead of auto-redirecting away.
- [x] Add websocket error/reconnect UI for live logs.
- [x] Group app actions into deploy, runtime, settings, and destructive areas.
- [x] Disable rollback when no rollback target exists or when rollback is unsupported.
- [x] Normalize frontend errors through actionable `Notice` UI instead of raw text or HTML.
- [x] Verify the Next.js/React frontend has no unsafe HTML sinks, no client-bundled secrets beyond intended `NEXT_PUBLIC_*` URLs, and no open redirects from provider-controlled values.
- [x] Fix mobile safe-area/nav issues and the missing `compact` button styling.

## Phase 5: Docs And Release Drift

- [x] Rewrite `README.md` to describe both self-hosted Hostlet and Hostlet Cloud accurately.
- [x] Refresh `docs/README.md`, `docs/ARCHITECTURE.md`, `docs/SECURITY.md`, `docs/FEATURE_GAPS.md`, and `docs/RELEASE_0.4.0.md`.
- [x] Add `docs/THREAT_MODEL_0.4.0.md` covering browser/API, GitHub App, Stripe, Cloudflare, API/agent, agent/Docker/Caddy, customer app container, webhook, log, and env-var trust boundaries.
- [x] Document the 0.4.0 sensitive-code ownership review for `auth`, `crypto`, `github_app`, `web`, `deploy`, `agent`, migrations, and infra; note that single-maintainer ownership is expected for Shane's personal project and must be offset by focused pre-tag review.
- [x] Remove stale claims that Hostlet is local-only.
- [x] Document `hostlet.cloud`, `*.hostlet.cloud`, GitHub App cloud auth, Device Flow self-host auth, Stripe sandbox billing, Cloudflare wildcard DNS, and deferred cloud features.
- [x] Clean or ignore stale `dist/` release artifacts so `0.3.11` metadata is not shipped as part of 0.4.0.
- [x] Update docs around private exposure, `/data`, Compose rollback, Caddy direct-origin behavior, and Cloudflare tunnel behavior.

## Phase 6: Testing And Release Gates

- [x] Add Rust API tests for cloud auth gates, revoked sessions, ownership isolation, Stripe webhooks, GitHub App install handling, unsupported cloud restrictions, and operator-only endpoint access.
- [x] Add agent tests for Caddy route reload failure and Compose validation.
- [x] Add Postgres migration/API smoke coverage in CI for cloud mode.
- [x] Add web smoke or Playwright coverage for login/setup states, create app, deploy logs, settings, cloud/self-host mode differences, disabled setup reasons, and security headers in production mode.
- [x] Expand `docs/VALIDATION_0.4.0.md` into separate self-hosted, cloud-local, `hostlet.cloud` infra, upgrade/rollback, backup/restore, release artifact, and manual paid-deploy gates.
- [x] Add a pre-tag security gate that checks runtime headers, cookie flags for HTTP dev versus HTTPS cloud, no public source maps unless intentionally protected, and no raw secrets in logs, browser bundles, docs, or release artifacts.
- [x] Tighten release gating so tag publishing cannot bypass CI-equivalent checks.
- [x] Run responsive QA at 320, 375, 768, 1024, and desktop widths for nav, create app, app detail, env editor, settings, and deployment logs.

## Deferred After 0.4.0

These are explicit non-goals for the 0.4.0 final-polish release and should remain deferred until after tagging:

- Cloud custom domains.
- Cloud Compose support.
- Managed databases.
- Persistent disk upsells.
- Multi-worker scheduling.
- Production Stripe live mode.
- Advanced Cloudflare certificate setup for deeper `*.apps.hostlet.cloud` hostnames.

## Notes From Read-Only Exploration

- Backend/API risks centered on cloud ownership, billing gates, job isolation, GitHub App install state, Stripe subscription state, and session revocation.
- Runtime/infra risks centered on Caddy route atomicity, raw host port exposure, job lease duration, Compose rollback, Compose validation, and status reconciliation.
- UX risks centered on incomplete cloud/self-host copy separation, disabled navigation, deployment log behavior, action grouping, error rendering, and mobile polish.
- QA/release risks centered on stale docs, thin 0.4 validation, missing DB/cloud integration tests, release workflow gaps, and stale `dist/` artifacts.
- Security best-practice risks centered on browser-exposed secrets, unsafe error rendering, provider callback state validation, runtime security headers, cookie flags, and public release artifacts.
- Threat-model risks centered on customer-to-customer isolation, provider webhook trust, API-to-agent trust, agent-to-Docker privilege, Caddy route integrity, and customer app access to platform secrets.
- Ownership-map risk is single-maintainer sensitive code ownership; this is acceptable for this personal project only if the listed sensitive areas get an explicit pre-tag review.

## Assumptions

- 0.4.0 is a private hosted cloud beta that should be safe enough for real first users.
- Stripe remains in sandbox for this release.
- Cloud MVP remains single-service, Dockerfile/generated Node only.
- Self-hosted Compose remains supported, but unsafe or incomplete claims around rollback/private exposure must be fixed or documented.
- The long-running implementation agent should update this file as each checklist item is completed.
