# Hostlet 0.4.1 Implementation Report

## Summary

Hostlet 0.4.1 makes Hostlet Cloud auto-redeploy managed and always on for cloud apps. Self-hosted behavior remains opt-in and unchanged.

## Implemented Changes

- Cloud app creation now persists managed defaults: `public_exposure=true` and `auto_deploy=true`, regardless of legacy create payload values.
- Cloud create still rejects customer-controlled domains and Compose, but no longer rejects create-time `public_exposure` or `auto_deploy` fields.
- Cloud app updates still reject `public_exposure` and `auto_deploy`, preserving managed SaaS behavior with no opt-out UI.
- Cloud UI now shows managed auto-redeploy on create summary, app list, app detail, and dashboard surfaces while keeping editable toggles hidden.
- Package versions for API, agent, and CLI are bumped to `0.4.1`.

## Validation

- API tests cover cloud create validation for managed exposure and auto-redeploy inputs.
- Cloud API E2E asserts created cloud apps return fixed cloud resources plus `publicExposure: true` and `autoDeploy: true`.
- Playwright coverage asserts cloud automation is visible as managed behavior while self-hosted controls remain editable.

## Production Follow-Up

After tagged images are published and deployed, backfill existing Hostlet Cloud app rows:

```sql
UPDATE apps
SET auto_deploy=true, updated_at=now()
WHERE auto_deploy=false
  AND domain LIKE '%.hostlet.cloud';
```

No DB migration is included because this backfill is intentionally scoped to the managed production cloud domain and must not flip self-hosted installs.
