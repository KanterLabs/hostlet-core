# Hostlet 0.3.4 Release Notes

Date: 2026-05-25

Hostlet 0.3.4 fixes Docker cleanup so it does not remove the Hostlet control plane while pruning old managed app containers.

## Fixes

- Skips Docker Compose managed containers during agent cleanup jobs, preserving API, web, local agent, database, Caddy, and tunnel containers.
