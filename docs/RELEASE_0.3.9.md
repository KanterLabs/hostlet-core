# Hostlet 0.3.9 Release Notes

Hostlet 0.3.9 adds a public GitHub repository inspection flow for self-hosting open source services with less manual setup.

## Highlights

- Inspect public GitHub URLs from the app creation page.
- Preview inferred runtime settings, ports, health path, warnings, and environment prompts before deployment.
- Create and immediately deploy inspected apps.
- Deploy public repositories without requiring a stored GitHub token.
- Add generated Compose runtime support for Hostlet-managed service definitions.
- Add a Gitea preset for the official rootless SQLite image with named volumes.
- Add a release guard that checks `v0.3.9` style tags match all Rust crate versions before publishing.

## Notes

Gitea HTTP is supported in this release. SSH Git access needs TCP routing support and remains out of scope for 0.3.9.
