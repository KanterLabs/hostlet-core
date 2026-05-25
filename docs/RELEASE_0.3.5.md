# Hostlet 0.3.5 Release Notes

Date: 2026-05-25

Hostlet 0.3.5 fixes the tagged release workflow on the self-hosted runner.

## Fixes

- Removes the passworded `sudo apt-get install unzip` step from the release workflow; the runner already provides `unzip`.
