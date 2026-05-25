# Hostlet 0.3.8 Release Notes

Date: 2026-05-25

Hostlet 0.3.8 adds constrained Docker Compose support for multi-service apps and expands deployment documentation.

## Added

- Compose runtime mode for apps that have one public web service and private supporting services.
- Repo-local `hostlet.yml` manifest for declaring the Compose file, web service, port, and health path.
- Compose validation that rejects unsafe or ambiguous fields such as host ports, `container_name`, host networking, privileged containers, devices, and host bind mounts.
- Compose deployment through the local agent with Hostlet-managed loopback port binding, health checks, Caddy routing, logs, cleanup, and runtime metadata.
- App create/detail runtime controls in the web UI.
- `docs/DEPLOYING_APPS.md` with Dockerfile, generated Node, and Compose examples.

## Notes

- Compose support is local-agent only.
- A Hostlet Compose app exposes one public HTTP route.
- Named volumes persist across redeploys and rollbacks. Hostlet does not roll volume or database state back.
