# Migration version reservations

Core migrations are applied from a merged tree: the hostlet-cloud overlay copies
its own `apps/api/migrations/` on top of the vendored hostlet-core directory at
Docker build time.  sqlx orders migrations by numeric version prefix and has no
duplicate-version detection, so a version collision crashes the API at startup
(checksum VersionMismatch on already-applied databases, primary-key violation on
fresh ones).

## Cloud-reserved versions

The following versions are **taken by hostlet-cloud** and must never be used in
core:

| Version | Cloud file |
|---------|-----------|
| 021 | `021_cloud_accounts.sql` |
| 022 | `022_simple_cloud_app_limits.sql` |
| 024 | `024_cloud_portfolios.sql` |
| 027 | `027_cloud_usage_rollup_hardening.sql` |

## Next migration

Core's next migration must start at **028 or higher**.  Before picking a
version number, check `hostlet-cloud/apps/api/migrations/` for newly reserved
numbers; a cloud-side renumbering/CI collision gate is planned as a separate
task.

## Intentionally duplicated files

Versions **025** and **026** exist in both repositories as byte-identical
copies:

- `025_app_screenshots.sql`
- `026_resource_snapshot_numeric_metrics.sql`

Editing either of these files in core requires re-syncing the corresponding
copies in hostlet-cloud in the same change.
