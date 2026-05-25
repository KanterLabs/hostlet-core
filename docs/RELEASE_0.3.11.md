# Hostlet 0.3.11 Release Notes

Hostlet 0.3.11 makes private app URLs directly reachable from the Hostlet UI.

## Changes

- Publishes private app container ports on `0.0.0.0` instead of loopback only.
- Applies the same private bind behavior to Hostlet-generated Compose overrides.
- Adds `HOSTLET_PRIVATE_APP_HOST` for the address shown in private app links.
- Shows Visit actions for private apps using the current deployment's published port.

