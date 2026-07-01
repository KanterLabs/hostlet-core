# Migration version reservations

Core migrations are applied from a merged tree: the hostlet-cloud overlay copies
its own `apps/api/migrations/` on top of the vendored hostlet-core directory at
Docker build time.  sqlx orders migrations by numeric version prefix and has no
duplicate-version detection, so a version collision crashes the API at startup
(checksum VersionMismatch on already-applied databases, primary-key violation on
fresh ones).

## Reserved versions

The following versions are already taken across the merged core + cloud tree and
must never be reused. Cloud-owned versions in particular must never be used in
core (the overlay copies cloud's `apps/api/migrations/` on top of the vendored
core directory at Docker build time):

| Version | Repo  | File |
|---------|-------|------|
| 021 | cloud | `021_cloud_accounts.sql` |
| 022 | cloud | `022_simple_cloud_app_limits.sql` |
| 024 | cloud | `024_cloud_portfolios.sql` |
| 027 | cloud | `027_cloud_usage_rollup_hardening.sql` |
| 029 | cloud | `029_cloud_portfolio_order.sql` |
| 031 | cloud | `031_cloud_compose_plan_limits.sql` |
| 033 | cloud | `033_cloud_volume_storage_limit.sql` |
| 034 | cloud | `034_cloud_profile_entries.sql` |
| 036 | cloud | `036_cloud_total_storage_limit.sql` |
| 037 | cloud | `037_cloud_profile_entries_skills_kind.sql` |
| 038 | cloud | `038_apps_domain_unique.sql` |
| 039 | cloud | `039_cloud_tos_acceptance.sql` |
| 040 | core  | `040_apps_suspended_at.sql` |
| 041 | cloud | `041_cloud_subscription_last_event_at.sql` |
| 042 | cloud | `042_cloud_reserve_demo_subdomain.sql` |

## Next migration

Core's next migration must start at **043 or higher** unless a lower unused
number is intentionally coordinated with `hostlet-cloud`. Core already owns
`040` and cloud owns `038`, `039`, `041`, and `042` (see above), so `043` is the
next free version. Before picking a version number, check
`hostlet-cloud/apps/api/migrations/` for newly reserved numbers and keep
byte-identical duplicate migrations in sync.

A blocking cross-repo collision gate,
`hostlet-cloud/scripts/ci-cloud-migration-collision-gate.sh` (wired into cloud
CI), fails the build if core and cloud reserve the same version prefix.

## Intentionally duplicated files

Versions **025** and **026** exist in both repositories as byte-identical
copies:

- `025_app_screenshots.sql`
- `026_resource_snapshot_numeric_metrics.sql`

Editing either of these files in core requires re-syncing the corresponding
copies in hostlet-cloud in the same change.
