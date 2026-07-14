# Deploying Apps

Hostlet deploys supported GitHub repositories through the local self-hosted
agent. Remote self-hosted server registration is deferred in Hostlet Core;
managed worker behavior is outside the public Core runtime.

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

Railpack builds use a BuildKit container. Set `HOSTLET_RAILPACK_BUILDKIT_KEEPALIVE=true`
to keep it warm between builds; with keepalive enabled, Hostlet stops it after
`HOSTLET_RAILPACK_BUILDKIT_IDLE_SECONDS` of no Railpack builds (default `1800`). Set
`HOSTLET_RAILPACK_BUILDKIT_MEMORY_LIMIT_MB` to cap the BuildKit container memory.
After a cold start, Hostlet waits up to `HOSTLET_RAILPACK_BUILDKIT_READY_TIMEOUT_SECS`
(default `30`) for the BuildKit daemon to become ready before building. The managed
BuildKit container is not privileged by default; set
`HOSTLET_RAILPACK_BUILDKIT_PRIVILEGED=true` only for hosts that explicitly require
the old privileged backend.

## Compose Apps

Self-hosted Compose app support is intentionally constrained.

Allowed Compose behavior focuses on app services and named volumes. Hostlet rejects unsafe or ambiguous fields such as:

- host ports
- host networking
- custom networks
- privileged containers
- devices
- absolute or escaping host bind mounts
- `container_name`

Relative bind mounts that stay inside the repository may be remapped into
Hostlet-managed named volumes during deploy. Hostlet does not allow arbitrary
host paths or Docker socket mounts inside app services.

Docker Compose apps are supported only in self-hosted installs that meet the local safety checks.

In app detail, Compose apps show per-service cards. The web service is the
publicly routed service; backing services stay internal and report service
status, health, image/container metadata, and ports when available.

## Apps Page

The Apps page is a fleet view, not only a deploy launcher. It filters apps by
active, failed, public, healthy, degraded, unhealthy, and unknown states. App
cards show deployment status, health, machine, route, runtime, resource limits,
auto redeploy state, latest webhook result, app detail links, deployment logs,
and public/private app links when available.

## Runtime Health

The agent checks the current app container on its configured health path. HTTP `2xx` and `3xx` responses are healthy.

Health state is separate from deployment status:

- one failure marks an app degraded
- repeated failures mark it unhealthy
- owners can trigger a manual check or restart

The app detail page also shows recent health events, live Docker resource
statistics, storage usage, screenshots, automation/webhook status, settings,
and encrypted environment variable controls.

## Deployment Logs UI

Deployment detail shows queue/building position, ordered status steps, retained
logs, deployment metrics, websocket stream status, and first-error highlighting
for failed deployments. The log viewer supports copy, wrap/no-wrap, follow, and
jump-to-latest controls. If the live websocket disconnects, the page falls back
to retained logs and shows the stream state.

## Rollbacks And State

Rollback health-checks a previous successful release before atomically updating
routing. The current release remains available until that switch commits.

Single-service apps receive a stable Docker volume mounted at `/data`. Redeploys and rollbacks reuse that volume.

Compose apps keep backing services, their stable internal network, and declared
named volumes under a per-app project. Each web release runs in a separate
release project, so a failed deploy or rollback cannot tear down databases or
caches. Changes to the backing-service specification require explicit approval
from the failed deployment page before Hostlet mutates that stable layer.

For the crash and retry guarantees behind activation, see
[Deployment Reliability](reliability.md).
