# Hostlet 0.3.7 Release Notes

Date: 2026-05-25

Hostlet 0.3.7 fixes stale version text in the web dashboard.

## Fixes

- Reads the overview Release state version from `/api/system/version` instead of rendering a hardcoded `0.2.0`.
- Removes stale `0.2.0` copy from machine pages.
