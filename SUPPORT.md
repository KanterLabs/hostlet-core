# Hostlet Core Support

Hostlet Core is pre-1.0 software maintained on a best-effort basis. There is no
response-time or uptime SLA.

## Before Opening An Issue

1. Run `hostlet doctor` and review the relevant
   [troubleshooting guidance](docs/operations.md#troubleshooting).
2. Search [existing issues](https://github.com/KanterLabs/hostlet-core/issues).
3. Reproduce the problem on the latest published release when practical.

For a Core bug or setup problem, open a
[support issue](https://github.com/KanterLabs/hostlet-core/issues/new/choose)
with:

- Hostlet version.
- Linux distribution, kernel version, CPU architecture, and
  `getconf GNU_LIBC_VERSION` output.
- Docker Engine and Docker Compose versions.
- Access mode and exact reproduction steps.
- Redacted `hostlet doctor` output or logs.

Never include credentials, environment files, private repository contents,
provider identifiers, or customer data. Follow [SECURITY.md](SECURITY.md) for a
suspected vulnerability instead of opening a public issue.

Hostlet Cloud account, billing, or managed-service questions should go to
`support@hostlet.cloud`, not the Core issue tracker.
