# Deploying Apps

Hostlet deploys supported GitHub repositories through the local or managed agent.

## Dockerfile Apps

If the repository has a Dockerfile, Hostlet builds it and runs the resulting image.

The app should:

- listen on the configured HTTP port
- expose a health path that returns `2xx` or `3xx`
- avoid printing secrets in logs
- write persistent runtime data under `/data` when it needs Hostlet-managed persistence

## Generated Apps

If no Dockerfile exists, Hostlet can generate a Railpack deployment for common app shapes, including Node package managers, Bun, Python, Go, Rust, static sites, and supported web frameworks.

Generated apps run as a non-root user and receive `/data` for persistent app data when applicable.

## Compose Apps

Self-hosted Compose app support is intentionally constrained.

Allowed Compose behavior focuses on app services and named volumes. Hostlet rejects unsafe or ambiguous fields such as:

- host ports
- host networking
- custom networks
- privileged containers
- devices
- host bind mounts
- `container_name`

Docker Compose apps are supported only in self-hosted installs that meet the local safety checks.

## Runtime Health

The agent checks the current app container on its configured health path. HTTP `2xx` and `3xx` responses are healthy.

Health state is separate from deployment status:

- one failure marks an app degraded
- repeated failures mark it unhealthy
- owners can trigger a manual check or restart

## Rollbacks And State

Single-service rollback updates routing to a previous successful deployment. It does not delete containers or images.

Single-service apps receive a stable Docker volume mounted at `/data`. Redeploys and rollbacks reuse that volume.

Compose rollback is disabled for the current release. Compose apps keep their declared named volumes, and Hostlet does not inject `/data` into arbitrary Compose services.
