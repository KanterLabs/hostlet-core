# Security

Hostlet controls Docker deployments and should be treated as infrastructure software. Run it on a trusted machine, use strong secrets, and restrict access to the control plane.

## Current Security Controls

- First-run control-plane password with Argon2 hashing.
- Unlock cookie separate from GitHub login.
- GitHub OAuth Device Flow for self-hosted browser login and repository access.
- GitHub OAuth redirect plus GitHub App installation verification for Hostlet Cloud.
- GitHub access tokens encrypted at rest with AES-256-GCM.
- App environment variables encrypted at rest.
- HMAC-signed browser sessions.
- Optional `HOSTLET_ALLOWED_GITHUB_LOGINS` allowlist.
- CSRF/origin checks for browser state-changing requests.
- Security headers on API and web responses.
- GitHub webhook signature verification.
- Webhook replay resistance with GitHub delivery ID dedupe.
- Stripe webhook signature verification, timestamp tolerance, and event dedupe in cloud mode.
- In-memory rate limiting for setup, unlock, GitHub Device Flow, agent registration/events, agent WebSocket connection attempts, and GitHub webhooks.
- Agent authentication with hashed agent tokens.
- Signed deployment jobs verified by the agent.
- Agent-originated logs are size-limited.
- API does not mount the Docker socket.
- Deployed containers use `no-new-privileges`, dropped Linux capabilities, PID limits, loopback-only port publishing, and optional CPU/memory limits.
- Cloudflare DNS management is constrained to Hostlet-prefixed subdomains under the configured base domain.
- Public app exposure is per-app and can be opened or closed at runtime.
- Cloud customer compute is gated by cloud session, GitHub App installation, and active/trialing Stripe subscription state.
- Cloud apps do not receive worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, direct database access, or direct job-queue access.

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
- Hostlet Cloud: expose only intended HTTPS ingress for `hostlet.cloud` and `*.hostlet.cloud`; keep Postgres, Docker, Caddy admin, and raw Docker-published app ports private or loopback-only.

Do not expose the Docker socket, Postgres, or Caddy admin endpoint publicly.

## GitHub Device Flow

Hostlet uses GitHub OAuth Device Flow, so self-hosted installs do not need a redirect URI, callback URL, or OAuth client secret. Configure a GitHub OAuth App with Device Flow enabled and set `GITHUB_CLIENT_ID`.

When `HOSTLET_ALLOWED_GITHUB_LOGINS` is set, only those GitHub logins can create or use Hostlet sessions. Existing accounts not in the allowlist are rejected on login.

## Hostlet Cloud Auth and Billing

Hostlet Cloud uses GitHub OAuth redirect login and a GitHub App installation flow. The GitHub App install callback must validate state, and installation ownership must be verified before an installation is associated with a cloud user. Organization installs require owner/admin access.

Stripe remains in sandbox for 0.4.0. Checkout completion alone does not grant indefinite compute. Subscription created/updated/deleted webhooks are the authoritative source for active, trialing, cancelled, and deleted subscription state.

Cloud app create, deploy, restart, rollback, env mutation, job retry/cancel, and runtime mutation require an active cloud session, GitHub App installation, and active/trialing subscription. Operator-only cleanup remains unavailable to cloud customers.

## Cloudflare DNS Safety

Hostlet only manages app records when all are true:

- `HOSTLET_BASE_DOMAIN` is configured.
- The app domain ends with `.<HOSTLET_BASE_DOMAIN>`.
- The app domain has a single label before the base domain.
- The app domain does not include a port.
- The app label is not reserved, including `www`, `mail`, `api`, `hostlet`, or other common infrastructure names.
- The Cloudflare CNAME is owned by the requesting app in `app_public_dns_records`, is unclaimed in Cloudflare, or is an old `HOSTLET_DOMAIN_PREFIX` legacy record.

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

Hostlet Cloud workers should be treated as privileged infrastructure. Customer app containers are untrusted and must not receive platform secrets, worker control tokens, provider secrets, or direct network access to platform control surfaces.

## Secret Handling

The agent redacts obvious secret-looking command output, but log redaction is not a substitute for safe app behavior. Avoid printing secrets in build scripts and runtime logs.

Back up `ENCRYPTION_KEY`. If it is lost, encrypted GitHub tokens and app environment variables cannot be decrypted.

App containers receive a writable `/data` Docker volume. Treat `hostlet-app-data-*` volumes as application data: include them in backups, restrict host access to trusted administrators, and delete apps only when their persistent data can also be removed.

Single-service apps receive `/data` automatically. Compose apps keep their declared named volumes; Hostlet does not inject `/data` into arbitrary Compose services. Compose rollback is disabled for 0.4.0, so application data rollback remains the app/operator's responsibility.

## Known Gaps

See [FEATURE_GAPS.md](FEATURE_GAPS.md) for the full product and security backlog. Highest-priority security gaps:

- no audit log UI
- no role-based access control
- no durable rate-limit storage across API restarts
- backup/restore exists, but scheduled/off-host backup and clean-machine restore validation are still manual
- no automated dependency/image scanning pipeline
