# Security

Hostlet controls deployments and should be treated as infrastructure software. Run self-hosted Hostlet on trusted machines, use strong secrets, and restrict control-plane access.

## Current Controls

- First-run control-plane password with Argon2 hashing.
- Unlock cookie separate from GitHub login.
- GitHub OAuth Device Flow for self-hosted login and repository access.
- GitHub OAuth redirect plus GitHub App installation verification for Hostlet Cloud.
- GitHub access tokens and app environment variables encrypted at rest.
- HMAC-signed browser sessions.
- Optional GitHub login allowlist.
- CSRF/origin checks for browser mutations.
- Security headers on API and web responses.
- GitHub webhook signature verification and delivery dedupe.
- Stripe webhook signature verification and event dedupe in cloud mode.
- Agent authentication with hashed agent tokens.
- Signed deployment jobs verified by the agent.
- API does not mount the Docker socket.
- App containers use loopback-only port publishing and reduced privileges where Hostlet controls the runtime.
- Cloud apps do not receive platform secrets or direct platform control access.

## Required Production Environment

Set strong non-default values before exposing Hostlet:

```text
HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false
ENCRYPTION_KEY=<base64 32-byte key>
SESSION_SECRET=<long random value>
JOB_SIGNING_SECRET=<long random value>
LOCAL_AGENT_TOKEN=<long random value>
GITHUB_WEBHOOK_SECRET=<long random value>
HOSTLET_SETUP_TOKEN=<long random value>
HOSTLET_ALLOWED_GITHUB_LOGINS=<owner login>
```

Do not commit `.env`, `.env.prod`, private keys, tokens, or provider credentials.

## Hostlet Cloud Security Model

The repo is open source, but Hostlet Cloud is operated as a managed SaaS.

- Public docs can describe architecture and controls.
- Public docs must not expose exact production inventory, internal-only IPs, provider IDs, backup paths, raw environment files, or secret values.
- Provider credentials stay in Hostlet-operated infrastructure.
- Customer apps never receive worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, direct database access, or direct job-queue access.
- Managed workers are privileged infrastructure because they control Docker and Caddy.

## Threat Model Summary

Highest-risk boundaries:

- browser/API session and tenant isolation
- GitHub App installation ownership
- Stripe subscription state and webhook trust
- API-to-agent job signing
- agent/Docker/Caddy host privileges
- customer app container isolation
- Cloudflare DNS and routing ownership
- app env vars and deployment logs

Important security objectives:

- prevent account takeover and tenant mixups
- prevent unpaid managed compute
- prevent provider credential disclosure
- prevent unauthorized host-level jobs
- prevent stale or incorrect public routes
- prevent accidental secret leakage in logs, docs, and artifacts

## Remaining Risks

- Docker socket access remains highly privileged.
- No custom seccomp/AppArmor profile is configured beyond Docker defaults.
- No per-app network egress policy exists.
- User-provided Dockerfiles are responsible for their own runtime user.
- Image vulnerability scanning is not yet integrated.
- Durable distributed rate limiting and audit UI are not yet complete.
- Scheduled off-host backups and clean-machine restore validation remain operational work.

Use separate machines or VMs for higher-risk workloads.
