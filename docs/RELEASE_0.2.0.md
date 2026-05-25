# Hostlet 0.2.0 Release Notes

Date: 2026-05-24

Hostlet 0.2.0 focuses on reliability after the first deployment and on making Hostlet itself easier to keep current.

## Highlights

- Recurring runtime health checks for deployed apps.
- Runtime health states: `healthy`, `degraded`, `unhealthy`, and `unknown`.
- App health snapshots and recent health event history.
- Health pills and filters on app list and dashboard.
- Runtime health panel on app detail with **Check now** and **Restart container**.
- Focused polling for dashboard, app list, app detail health, server state, and update state.
- Hostlet update detection through GitHub Releases.
- Optional `hostlet-release.json` release manifest support.
- Settings update panel with current/latest version, release notes, minimum supported version, and migration flags.
- CLI update flow:
  - `hostlet version`
  - `hostlet status`
  - `hostlet update check`
  - `hostlet update --dry-run`
  - `hostlet update`
  - `hostlet update rollback`
- Expanded `hostlet doctor` checks for Compose, services, disk space, backup freshness, update availability, and API health.
- Release workflow now publishes `hostlet-release.json` with the CLI binary and checksum.

## Upgrade Notes

Run from the Hostlet server:

```bash
hostlet update check
hostlet update --dry-run
hostlet update
```

`hostlet update` creates a pre-update backup by default, saves update state, verifies the CLI checksum, saves the previous CLI binary, replaces the CLI, restarts the Compose stack, and runs `hostlet doctor`.

Rollback restores the previous CLI binary, restores saved Compose files when available, and restarts services:

```bash
hostlet update rollback
```

Database rollback is not automatic. Keep the pre-update backup until the upgraded stack has been validated.

## Known Limits

- Remote VPS agents remain disabled.
- Deploy, rollback, restart, and check-now jobs are still delivered over the connected agent WebSocket rather than a durable queue.
- Automatic self-healing policies remain off; 0.2.0 provides manual check, restart, redeploy, and rollback actions.
- Runtime health event pruning runs when the API ingests health reports, keeping seven days of events and the latest 500 events per app.
- Release signing, SBOMs, and artifact attestations are still future work.
