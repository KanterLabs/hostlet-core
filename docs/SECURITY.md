# Security

Hostlet controls Docker deployments and should be treated as infrastructure software. Run it on a trusted machine, use strong secrets, and restrict access to the control plane.

## Current Security Controls

- First-run control-plane password with Argon2 hashing.
- Unlock cookie separate from GitHub login.
- GitHub OAuth Device Flow for browser login and repository access.
- GitHub access tokens encrypted at rest with AES-256-GCM.
- App environment variables encrypted at rest.
- HMAC-signed browser sessions.
- Optional `HOSTLET_ALLOWED_GITHUB_LOGINS` allowlist.
- CSRF/origin checks for browser state-changing requests.
- Security headers on API and web responses.
- GitHub webhook signature verification.
- Webhook replay resistance with GitHub delivery ID dedupe.
- Agent authentication with hashed agent tokens.
- Signed deployment jobs verified by the agent.
- Agent-originated logs are size-limited.
- API does not mount the Docker socket.
- Deployed containers use `no-new-privileges`, dropped Linux capabilities, PID limits, loopback-only port publishing, and optional CPU/memory limits.
- Cloudflare DNS management is constrained to Hostlet-prefixed subdomains under the configured base domain.
- Public app exposure is per-app and can be opened or closed at runtime.

## Required Production Environment

Set these before exposing Hostlet:

```bash
HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false
ENCRYPTION_KEY=<base64 32-byte key>
SESSION_SECRET=<long random value>
JOB_SIGNING_SECRET=<long random value>
LOCAL_AGENT_TOKEN=<long random value>
GITHUB_WEBHOOK_SECRET=<long random value>
HOSTLET_SETUP_TOKEN=<long random value>
HOSTLET_ALLOWED_GITHUB_LOGINS=your-github-login
```

Generate values:

```bash
openssl rand -base64 32
openssl rand -hex 32
```

Use a non-default Postgres password and avoid exposing Postgres publicly.

## Network Guidance

Recommended exposure:

- Web UI: trusted LAN, VPN, or HTTPS-protected public host.
- API: same trusted boundary as the UI.
- Postgres: loopback/container network only.
- Agent: no inbound public HTTP required.
- Caddy app router: loopback only when using Cloudflare Tunnel.
- Cloudflare Tunnel: optional for public app traffic.

Do not expose the Docker socket, Postgres, or Caddy admin endpoint publicly.

## GitHub Device Flow

Hostlet uses GitHub OAuth Device Flow, so self-hosted installs do not need a redirect URI, callback URL, or OAuth client secret. Configure a GitHub OAuth App with Device Flow enabled and set `GITHUB_CLIENT_ID`.

When `HOSTLET_ALLOWED_GITHUB_LOGINS` is set, only those GitHub logins can create or use Hostlet sessions. Existing accounts not in the allowlist are rejected on login.

## Cloudflare DNS Safety

Hostlet only manages app records when all are true:

- `HOSTLET_BASE_DOMAIN` is configured.
- The app domain ends with `.<HOSTLET_BASE_DOMAIN>`.
- The app domain has a single label before the base domain.
- That label starts with `HOSTLET_DOMAIN_PREFIX`.

This prevents Hostlet from deleting or changing the apex domain, portfolio hostnames, `www`, or unrelated records.

## App Container Risks

Deployed apps are untrusted code. The local agent builds and runs them through Docker and therefore has host-level impact if Docker is compromised.

Current hardening is useful but incomplete:

- Docker socket access remains highly privileged.
- No seccomp/AppArmor profile is configured beyond Docker defaults.
- No per-app network egress policy exists.
- Generated Node images run as a non-root user, but user-provided Dockerfiles are trusted to define their own runtime user.
- No image vulnerability scanning is integrated.

Use separate machines or VMs for higher-risk workloads.

## Secret Handling

The agent redacts obvious secret-looking command output, but log redaction is not a substitute for safe app behavior. Avoid printing secrets in build scripts and runtime logs.

Back up `ENCRYPTION_KEY`. If it is lost, encrypted GitHub tokens and app environment variables cannot be decrypted.

## Known Gaps

See [FEATURE_GAPS.md](FEATURE_GAPS.md) for the full product and security backlog. Highest-priority security gaps:

- no backup/restore tooling
- no audit log UI
- no role-based access control
- no rate limiting
- incomplete production deployment packaging
- no automated dependency/image scanning pipeline
