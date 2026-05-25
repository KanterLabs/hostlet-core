# Hostlet 0.3.10 Release Notes

Hostlet 0.3.10 fixes Compose app deployments from the local agent.

## Fixes

- Bundles Docker Compose v2 into the production agent image.
- Logs Docker and Docker Compose availability when the agent starts.
- Fails Compose deployments with a clear message when Docker Compose v2 is unavailable.

This fixes generated Compose deployments such as the Gitea one-click flow added in 0.3.9.
