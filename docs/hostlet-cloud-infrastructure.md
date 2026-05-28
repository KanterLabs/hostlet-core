# Hostlet Cloud Infrastructure

This runbook tracks the public-safe operating model for the hosted Hostlet Cloud service at `hostlet.cloud`.

Hostlet is open source, so this file must be safe to publish. Do not add secret values, internal-only IPs, provider resource IDs, tunnel IDs, private backup paths, raw `.env.prod` contents, private keys, tokens, account IDs, or password-reset style operational shortcuts. Keep exact production inventory in the ignored private tracker described below.

## Current Public State

- Service: Hostlet Cloud private beta.
- Public control plane: `https://hostlet.cloud`.
- Managed app hosts: `*.hostlet.cloud`.
- Current production release: `v0.5.0` after release promotion.
- Current release commit: record the tagged release commit during deployment.
- Production runtime model: tagged GHCR images, not build-on-VM containers.
- Production image registry: `ghcr.io/shanekanterman04`.
- Production compose project name: `infra`.
- Production mode: `HOSTLET_MODE=cloud`.

Current release images:

| Service | Image |
| --- | --- |
| API | `ghcr.io/shanekanterman04/hostlet-api:v0.5.0` |
| Web | `ghcr.io/shanekanterman04/hostlet-web:v0.5.0` |
| Managed agent | `ghcr.io/shanekanterman04/hostlet-agent:v0.5.0` |

## Public Topology

Hostlet Cloud runs the hosted web panel, API, Postgres database, Caddy router, Cloudflare Tunnel connector, managed Hostlet agent, and customer app containers on managed Hostlet infrastructure.

Public traffic enters through Cloudflare, reaches the Hostlet Cloud ingress, and is routed by Caddy:

- `hostlet.cloud` routes to the hosted web/API services.
- `*.hostlet.cloud` routes to managed customer app containers through Hostlet-managed Caddy snippets.
- Raw app ports, Postgres, Docker, Caddy admin, and platform control surfaces must not be publicly reachable.

The managed agent is privileged infrastructure because it controls Docker and Caddy. Customer apps are untrusted and must never receive worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, direct database access, direct job-queue access, or other platform credentials.

## Secret And Inventory Policy

Public repo files may record required configuration key names and behavior, but never values.

Cloud-only secret categories:

- GitHub OAuth and GitHub App credentials.
- GitHub webhook secret.
- Stripe API and webhook secrets.
- Cloudflare API and tunnel credentials.
- Hostlet setup/session/encryption/job-signing/agent secrets.
- Postgres credentials.
- Provider resource IDs and account IDs when they materially identify the production deployment.

Exact production inventory belongs in:

```text
docs/private/hostlet-cloud-infrastructure.md
```

That path is gitignored. It should still avoid raw secret values; use presence, rotation dates, and external secret-store references instead.

## Release And Deploy Procedure

1. Merge release-ready work to `main`.
2. Wait for required CI gates to pass.
3. Create a version tag such as `v0.5.0`.
4. Ensure the release publishes:
   - CLI binary and checksum.
   - `hostlet-release.json`.
   - GHCR images for API, web, and agent.
5. Verify the release manifest includes the intended image tag and non-empty image digests.
6. On production, set `HOSTLET_IMAGE_TAG` to the new tag in the cloud environment file.
7. Pull tagged images and restart the image-based production compose stack.
8. Verify health, pricing, authenticated `/api/system/version`, service state, and image tags before considering the deploy complete.

Production should not build application control-plane images on the VM for normal upgrades. The VM should consume tagged release images.

## Verification Checklist

Run these checks after every cloud infrastructure or release change:

```bash
curl -fsS https://hostlet.cloud/health
curl -fsSI https://hostlet.cloud/pricing
# With an operator browser session or copied session cookie:
curl -fsS -H "cookie: <session-cookie>" https://hostlet.cloud/api/system/version
# With the production operator token from the secret store:
curl -fsS -H "x-hostlet-agent-token: <operator-token>" https://hostlet.cloud/api/system/operator-status
```

Confirm:

- `/health` returns `ok`.
- `/pricing` returns HTTP `200`.
- Authenticated `/api/system/version` reports the expected `currentVersion`.
- Operator status reports the expected version, runtime mode, image tag/registry, database connectivity, server counts, health counts, and public app route count.
- API, web, managed agent, Postgres, Caddy, and cloudflared are running.
- API, web, and managed agent are using the expected `vX.Y.Z` GHCR images.
- The release manifest image digests match the image refs.
- Caddy routes `hostlet.cloud` to the control plane and wildcard app hosts to managed app snippets.
- Cloudflare tunnel/connectivity is healthy.
- No raw Docker app ports, Postgres ports, Docker socket, Caddy admin endpoint, or internal control endpoints are exposed publicly.

## Incident And Rollback Notes

- Keep a pre-cutover backup before replacing a production release directory.
- Roll back by restoring the previous release directory/environment pairing and restarting the compose project with the previous `HOSTLET_IMAGE_TAG`.
- Do not delete persistent Postgres or agent volumes during an application rollback.
- If API startup fails after an env change, check for database credential mismatch before assuming a code regression.
- If routing fails, check Caddy syntax/reload behavior and preserve the previous working app snippet state.

Record exact backup paths, VM access notes, provider resource IDs, and rollback command transcripts only in the ignored private tracker.

## Change Log

| Date | Change |
| --- | --- |
| 2026-05-28 | Planned consolidated `v0.5.0` release includes the build packaging and cloud beta UX work that had been tracked through `0.6.0`. |
| 2026-05-28 | Production confirmed on the GCP VM and updated to tagged GHCR release images for `v0.4.1`. |
| 2026-05-27 | Production moved to tagged GHCR release images for `v0.4.0`; release metadata now records non-empty image digests. |

## Related Docs

- [Architecture](architecture.md)
- [Security](security.md)
- [Operations](operations.md)
- [Hostlet Cloud](hostlet-cloud.md)
