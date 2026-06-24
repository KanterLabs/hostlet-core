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
| 029 | `029_cloud_portfolio_order.sql` |
| 031 | `031_cloud_compose_plan_limits.sql` |
| 033 | `033_cloud_volume_storage_limit.sql` |
| 034 | `034_cloud_profile_entries.sql` |
| 036 | `036_cloud_total_storage_limit.sql` |
| 037 | `037_cloud_profile_entries_skills_kind.sql` |

## Next migration

Core's next migration must start at **038 or higher** unless a lower unused
number is intentionally coordinated with `hostlet-cloud`. Before picking a
version number, check `hostlet-cloud/apps/api/migrations/` for newly reserved
numbers and keep byte-identical duplicate migrations in sync.

## Intentionally duplicated files

Versions **025** and **026** exist in both repositories as byte-identical
copies:

- `025_app_screenshots.sql`
- `026_resource_snapshot_numeric_metrics.sql`

Editing either of these files in core requires re-syncing the corresponding
copies in hostlet-cloud in the same change.
