# Hostlet 0.4.0 Validation

Use this checklist before pushing implementation work and before tagging `v0.4.0`. Do not paste secret values into logs, docs, issue comments, or release artifacts. When validating `.env`, `.env.prod`, or `infra/.env`, record only whether required values are present, missing, empty, or placeholder-looking.

## Automated Gate

Run from the repository root:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/hostlet-target cargo check --workspace
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test -p hostlet-api
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test -p hostlet-agent
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
docker compose -f infra/docker-compose.prod.yml config
HOSTLET_CADDYFILE=./Caddyfile.direct docker compose -f infra/docker-compose.prod.yml config
```

## Self-Hosted Regression Gate

1. Confirm `hostlet version` reports `0.4.0`.
2. Run setup and unlock with `HOSTLET_MODE=self_hosted` or unset.
3. Connect GitHub through Device Flow.
4. Create and deploy a local single-service app.
5. Confirm deployment logs stay visible after success and include a clear app-detail CTA.
6. Confirm runtime health, manual check-now, manual restart, rollback, resource stats, publish/private URL, and delete still work.
7. Confirm local deploys do not require a Hostlet Cloud account, Stripe subscription, or GitHub App installation.
8. Confirm self-hosted Compose apps validate accepted named volumes and reject host ports, custom networks, privileged fields, and bind mounts.
9. Confirm Compose rollback is disabled or clearly labeled as unsupported for `0.4.0`.
10. Confirm `/data` is mounted as a stable Hostlet-managed Docker volume and survives redeploys.

## Cloud-Local Gate

1. Run the API and web UI with `HOSTLET_MODE=cloud`.
2. Confirm `/api/system/version` reports `mode: cloud`.
3. Confirm `/api/cloud/status` reports GitHub App and Stripe configuration presence without exposing values.
4. Confirm migration `021_cloud_accounts.sql` creates cloud users, sessions, GitHub installations, Stripe records, entitlements, usage buckets, and webhook dedupe tables.
5. Confirm cloud users cannot create apps, deploy, restart, retry jobs, cancel jobs, mutate env vars, or run cleanup without both GitHub App installation and active subscription state.
6. Confirm checkout completion alone creates only pending billing state; subscription created/updated webhooks are required for active or trialing compute.
7. Confirm subscription cancellation/deletion removes compute eligibility.
8. Confirm two cloud users cannot see or control each other's apps, deployments, logs, jobs, env vars, or billing state.
9. Confirm cloud create/update rejects Compose, custom domains, public/private toggles, auto-deploy toggles, and arbitrary CPU/RAM edits.
10. Confirm GitHub App install callback rejects missing, expired, or mismatched state.

## `hostlet.cloud` Infra Gate

1. Confirm `hostlet.cloud` points through Cloudflare to the reserved Hostlet VM ingress.
2. Confirm `*.hostlet.cloud` points through Cloudflare to the reserved Hostlet VM ingress.
3. Confirm firewall policy allows intended public ingress only on 80/443.
4. Confirm raw Docker-assigned app ports are not publicly reachable.
5. Confirm Caddy routes `hostlet.cloud` to the web/API services and wildcard app hostnames to managed app snippets.
6. Confirm failed Caddy reload restores the previous route snippet and does not activate a failed deployment later.
7. Confirm `cloudflared` starts only when the tunnel profile/token is intentionally configured.

## Paid Deploy Gate

Using Stripe sandbox only:

1. Sign in to `hostlet.cloud` as a real GitHub user.
2. Install the Hostlet GitHub App for an allowed personal account or an org where the user has admin rights.
3. Complete Stripe sandbox checkout for the intended plan.
4. Confirm Stripe webhook delivery records the subscription as active or trialing.
5. Create one supported cloud app from a GitHub repo.
6. Deploy it, view logs, restart it, roll back where supported, and delete it.
7. Confirm app limit and starter resource entitlements are enforced.

## Upgrade And Rollback Gate

1. Start from the latest released `0.3.x` install.
2. Run `hostlet update --dry-run` and confirm release metadata, checksums, and Compose/database migration flags are sane.
3. Run `hostlet update` with a pre-update backup enabled.
4. Confirm API, web, agent, Caddy, and cloudflared services restart successfully.
5. Confirm `hostlet doctor` and `hostlet status` pass.
6. Run `hostlet update rollback` and confirm the previous CLI and Compose files are restored.
7. Confirm database rollback remains manual from backup and is documented.

## Backup And Restore Gate

1. Run `scripts/backup.sh` against a working self-hosted install.
2. Confirm the backup includes a Postgres dump and agent state volume when available.
3. Confirm the backup does not copy `.env`, `.env.prod`, `infra/.env`, or raw secret values.
4. Restore on a clean machine or VM with the original `ENCRYPTION_KEY`.
5. Confirm GitHub tokens, app env vars, deployments, and app state decrypt and load.
6. Confirm a restored app can deploy and be deleted cleanly.

## Release Artifact Gate

1. Confirm `dist/` is ignored and stale `0.3.x` artifacts are not included in the working tree for release.
2. Build fresh `0.4.0` release artifacts from a clean commit.
3. Confirm `hostlet-release.json` says `0.4.0`, includes the correct checksums, and links to the `v0.4.0` notes.
4. Confirm SBOM/checksum files match the generated binary.
5. Confirm release artifacts contain no `.env`, `.env.prod`, `infra/.env`, private keys, webhook secrets, Stripe secrets, GitHub tokens, Cloudflare tokens, or app env values.
6. Confirm no public browser source maps are shipped unless intentionally protected.

## Security Gate

1. Confirm runtime security headers are present on API responses: frame denial, content type nosniff, referrer policy, and permissions policy.
2. Confirm cookies are `HttpOnly`, `SameSite=Lax`, scoped to `/`, and `Secure` on HTTPS cloud URLs.
3. Confirm browser-origin checks apply to browser mutations and explicitly exempt only machine-authenticated/provider-authenticated endpoints.
4. Confirm Stripe and GitHub webhooks rely on provider signatures and event dedupe, not CSRF/origin exemptions.
5. Confirm API, agent, web, docs, logs, and release artifacts contain no raw secret values.
6. Review `docs/THREAT_MODEL_0.4.0.md` and `docs/SECURITY_OWNERSHIP_0.4.0.md` before tag.

## Manual Responsive QA Gate

Check `/`, `/apps`, `/apps/new`, `/apps/[id]`, `/deployments/[id]`, `/logs`, `/settings`, and `/login` at 320, 375, 768, 1024, and desktop widths.

Confirm:

- Navigation and safe-area spacing are usable.
- Text does not overlap or overflow controls.
- Create-app disabled reasons are visible.
- Deployment logs remain readable.
- Env editor and settings forms fit on mobile.
- Cloud mode and self-hosted mode copy use the correct product terms.

## Final Release Rule

Do not tag `v0.4.0` until:

- The automated gate passes on the branch that will be tagged.
- Self-hosted, cloud-local, `hostlet.cloud` infra, paid deploy, upgrade/rollback, backup/restore, release artifact, security, and responsive QA gates are completed.
- The current plan file has no remaining unchecked `0.4.0` release-gate items except explicitly deferred after-0.4.0 work.
