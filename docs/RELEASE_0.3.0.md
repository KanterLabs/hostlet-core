# Hostlet 0.3.0 Release Notes

Date: 2026-05-25

Hostlet 0.3.0 starts the operations-hardening track described in `PLAN_0.3.0`.

## Highlights

- Durable agent-job metadata for claimed jobs, leases, attempts, payloads, results, and deployment links.
- Authenticated agent job claim and complete endpoints.
- Local agent polling for queued durable jobs.
- Deploy, rollback, delete, manual runtime health check, and restart actions now enqueue durable jobs instead of requiring immediate WebSocket delivery.
- API startup reconciliation for stale claimed/running agent jobs.
- Audit event storage plus owner-readable audit event API.
- Runtime health check and restart requests write audit events.
- Release workflow now emits SBOMs and GitHub build provenance attestations.
- Release workflow can publish detached GPG signatures for checksum files when signing secrets are configured.
- CLI update checks now report whether a release has a signed checksum asset.

## Known Limits

- Cleanup is owner-triggered; no background cleanup timer is enabled by default.
- Signed checksum verification is not yet enforced by the CLI; unsigned checksum verification remains supported.
- Remote VPS agents remain disabled.
