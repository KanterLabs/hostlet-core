# Security

Hostlet uses GitHub OAuth for user login. GitHub access tokens and app environment variables are encrypted at rest with AES-256-GCM using `ENCRYPTION_KEY`.

Browser sessions are HMAC-signed and expire. OAuth callbacks require a signed state cookie. In production, set `HOSTLET_ALLOWED_GITHUB_LOGINS` to the comma-separated GitHub logins allowed to access the control plane.

Agents authenticate with a unique server token generated during one-time registration. Install tokens are stored only as hashes and are cleared after registration.

GitHub webhook payloads are verified with `X-Hub-Signature-256`. Deployment jobs are signed by the control plane and verified by the agent before execution.

The agent does not expose public unauthenticated endpoints. It receives jobs over an authenticated WebSocket and reports events over authenticated HTTPS. The default local agent authenticates the same way as a remote VPS agent.

Deployment safety:

- New containers are built and health-checked before Caddy routing changes.
- The previous working container is preserved if a deployment fails.
- Rollback changes routing only to a previous successful deployment.
- Secret-looking log lines are redacted by the agent.

Production requirements:

- Use a random 32-byte base64 `ENCRYPTION_KEY`.
- Replace `LOCAL_AGENT_TOKEN`, `JOB_SIGNING_SECRET`, and `SESSION_SECRET` with long random values.
- Use long random webhook and job signing secrets.
- Do not set `HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=true` outside local development.
- Set `HOSTLET_ALLOWED_GITHUB_LOGINS` before exposing the service.
- Run the API behind HTTPS.
- Restrict VPS public firewall to ports 80 and 443 plus SSH.
