# Hostlet 0.1.0 Owner Input Plan

Date: 2026-05-21

Status update, 2026-05-22: this is a historical owner-input plan from before the updated `0.1.0` repackage. Current `0.1.0` is local-machine-only; remote VPS agents are disabled rather than shipped as beta. Current capabilities and remaining limits are summarized in [FEATURE_GAPS.md](FEATURE_GAPS.md) and [SHIP_PLAN_0.1.0.md](SHIP_PLAN_0.1.0.md).

This plan lists the things Codex cannot fully complete alone because they require external account access, a real repository, a domain/tunnel, a release decision, or a clean validation machine.

## Decisions Already Defaulted

No action needed unless you disagree:

- Auto-redeploy default: off. Users opt in per app/branch.
- Deploy concurrency: reject concurrent deploys for the same app in `0.1.0`; do not queue.
- App delete: remove all Hostlet-managed runtime resources for that app.
- Public tunnel exposure: off by default.
- Backup: local archive scripts for `0.1.0`.
- Remote VPS: disabled for the updated `0.1.0` package.
- Production packaging: Docker images plus production Compose.

## Needed From You

### 1. Canonical Repository URL

Needed for:

- remote agent install script
- production docs
- generated VPS install command

Recommended:

- Use the final public GitHub URL for this project.
- Obsolete for updated `0.1.0`: remote install commands are disabled, so `HOSTLET_REPO_URL` is not part of the release path.

What to provide:

- The final repo URL, or confirmation to keep `HOSTLET_REPO_URL` required.

### 2. GitHub OAuth Device Flow Confirmation

Needed for:

- final login smoke test
- README accuracy
- Device Flow validation

Recommended:

- Keep using the current Hostlet OAuth app for testing, with Device Flow enabled.
- For release, create/update one OAuth app and copy its Client ID into Hostlet.

What to provide:

- Confirm the testing OAuth app is still acceptable.
- For production release, provide final `PUBLIC_WEB_URL` and `PUBLIC_API_URL`.

### 3. Auto-Redeploy Test Repository

Needed for:

- real push-to-branch auto-redeploy acceptance test
- webhook delivery validation from GitHub

Recommended:

- Use a small disposable GitHub repo with `main` and one secondary branch.
- Allow Codex/CLI to push a harmless commit, or you push when asked.

What to provide:

- Repo name.
- Branch to auto-deploy.
- Whether Codex may push test commits from this machine.

### 4. GitHub Webhook Setup

Needed for:

- real GitHub webhook delivery test
- branch-push deploy validation

Recommended:

- Manual webhook setup is enough for `0.1.0`.
- Automatic webhook creation can wait until after `0.1.0`.

What to provide/do:

- Add webhook URL: `PUBLIC_API_URL/webhooks/github`
- Content type: `application/json`
- Secret: value of `GITHUB_WEBHOOK_SECRET`
- Events: push

If you want Codex to configure it:

- Provide `gh` CLI auth on this machine with permission to manage webhooks for the test repo, or provide a GitHub token with repo admin webhook permission.

### 5. Cloudflare Zone/Tunnel Validation

Needed for:

- final open/close tunnel smoke test
- DNS creation/deletion validation
- diagnostics validation

Recommended:

- Use a dedicated test zone or subdomain for validation.
- Use single-label app hostnames under the configured base domain, e.g. `runcomp.example.com`.
- Continue not touching apex/root records.

What to provide:

- Confirmation the current Cloudflare token/zone/tunnel are still valid for testing.
- Permission to create/delete app CNAME records under the chosen test domain.

### 6. Clean Install Validation Machine

Needed for:

- proving README setup from zero
- backup/restore validation on fresh volumes
- production Compose validation outside the current dev state

Recommended:

- A clean local Docker volume set is enough for first pass.
- A separate VM is better before tagging.

What to provide:

- Permission to create/remove test Docker volumes and containers locally, or access to a disposable VM.

### 7. Remote VPS Scope

Needed for:

- later re-enabling remote VPS agents after the local-machine release

Recommended:

- Do not include remote VPS support in the updated `0.1.0` package.
- Keep remote registration and install commands disabled until agent binaries, token rotation, uninstall/reinstall, and disposable VPS validation exist.

What to provide:

- Nothing for the updated `0.1.0` package.

### 8. Production Domain Choice

Needed for:

- final production docs
- GitHub Device Flow setup
- allowed origins
- Cloudflare tunnel domain setup

Recommended:

- Keep any personal domain for testing only.
- Use the real product domain for production once purchased.
- Keep portfolio or apex records untouched.

What to provide:

- Final production domain when ready.
- Whether API and web will share one host or use separate hosts.

Recommended production shape:

- Web: `https://hostlet.example.com`
- API: `https://hostlet-api.example.com`
- Apps: `https://<app>.example.com`

### 9. Release Versioning

Needed for:

- changelog
- release tag
- Docker image tags

Recommended:

- Tag as `v0.1.0`.
- Mark release as beta.
- Include known limitations in release notes.

What to provide:

- Confirmation to tag `v0.1.0` when release gates pass.

## Not Needed From You

Codex does not need external input to implement:

- app teardown
- app settings UI
- env var editor
- logout/reconnect UI
- deploy lock
- stale deployment recovery
- auto-redeploy code and local webhook simulation
- Cloudflare diagnostics code
- backup/restore scripts
- production Dockerfiles/Compose
- CI workflow
- docs updates

## Owner Acceptance Checklist

Before tagging `v0.1.0`, you or an approved external test should confirm:

1. GitHub OAuth works with the intended OAuth app.
2. A real GitHub push to the selected branch triggers auto-redeploy.
3. A push to a non-selected branch does not deploy.
4. Cloudflare open/close tunnel works and does not affect portfolio records.
5. Fresh install instructions work on a clean machine or clean Docker volumes.
6. Backup and restore work with the original `ENCRYPTION_KEY`.
7. Known limitations are acceptable for a beta release.
