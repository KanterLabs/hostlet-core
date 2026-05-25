# Hostlet 0.3.3 Release Notes

Date: 2026-05-25

Hostlet 0.3.3 fixes operator cleanup from the CLI.

## Fixes

- Allows `hostlet cleanup --dry-run` and `hostlet cleanup --yes` to call the operator cleanup endpoint with the local agent token without browser-origin headers.
