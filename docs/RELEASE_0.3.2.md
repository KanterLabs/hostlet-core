# Hostlet 0.3.2 Release Notes

Date: 2026-05-25

Hostlet 0.3.2 completes more of the 0.3 operations-hardening plan.

## Highlights

- Adds owner cleanup preview and run endpoints with explicit retention defaults.
- Adds durable `docker_cleanup` agent jobs with keep-list guardrails for current and previous successful deployments.
- Adds cleanup controls, recent durable jobs, retry/cancel actions, audit trail, and backup metadata display to Settings.
- Adds `hostlet cleanup --dry-run` and `hostlet cleanup --yes` through the operator API.
- Adds restore preflight checks for backup contents, `.env`, Docker, Compose, and disk space.
- Records latest backup metadata to `backups/latest.json` and, when available, the database settings table.
- Reconciles completed delete-app jobs from durable job payloads on API startup and job status checks instead of relying on an in-process finalizer task.
- Expands audit coverage for app create/update/delete, deploy, rollback, env var changes, public URL changes, cleanup, and job retry/cancel actions.

## Notes

- Cleanup is owner-triggered in 0.3.2; no background cleanup timer is enabled by default.
- Docker cleanup removes only Hostlet-managed containers/images that are not in the keep set supplied by the API.
