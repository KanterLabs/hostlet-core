# Hostlet Core

Hostlet Core is for developers comfortable operating a Linux server who want
to turn supported GitHub repositories into live apps without assembling build,
container, routing, health-check, and rollback tooling themselves. The control
plane and deployment worker run on infrastructure you control.

[Hostlet Cloud](https://hostlet.cloud) is the managed portfolio service;
Hostlet Core is the open-source engine you can run yourself. Billing, tenant
management, and hosted-service infrastructure are intentionally not part of
Core.

## Project Status

Hostlet Core is pre-1.0 beta software. It currently runs one control plane and
one local deployment agent on a single Linux host. Remote-host registration,
multi-host scheduling, high availability, and an SLA are not included.

Run repositories you trust, keep backups, and review the
[security guide](docs/security.md) before exposing Hostlet or its apps publicly.
Core is not a hardened isolation boundary for mutually untrusted tenants.

## What You Get

- GitHub-backed app deployment on your own Docker host.
- Dockerfile and Railpack-generated app support.
- Deployment logs, runtime health, restart, rollback, and delete flows.
- Encrypted app environment variables.
- App screenshot capture for exposed app URLs.
- Optional Cloudflare Tunnel support for public URLs.
- Digest-pinned release images from GHCR.

## Quick Start

Prerequisites:

- Linux x86_64 with glibc 2.39 or newer, such as Ubuntu 24.04 LTS or
  newer. Alpine/musl and older glibc distributions are not supported by the
  current CLI binary.
- Docker Engine with a reachable daemon and Docker Compose v2.
- Git and curl.
- GitHub OAuth App with Device Flow enabled.

Install the self-hosted CLI:

```bash
git clone https://github.com/KanterLabs/hostlet-core.git
cd hostlet-core
[ "$(uname -m)" = "x86_64" ] || { echo "Stable releases currently support Linux x86_64 only" >&2; exit 1; }
glibc_output="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
if [[ "$glibc_output" =~ ^glibc[[:space:]]+([0-9]+)\.([0-9]+)(\.[0-9]+)?$ ]]; then
  glibc_major="${BASH_REMATCH[1]}"
  glibc_minor="${BASH_REMATCH[2]}"
else
  echo "Stable releases require glibc 2.39 or newer" >&2
  exit 1
fi
(( glibc_major > 2 || (glibc_major == 2 && glibc_minor >= 39) )) || {
  echo "Stable releases require glibc 2.39 or newer (found $glibc_major.$glibc_minor)" >&2
  exit 1
}
curl -fLO https://github.com/KanterLabs/hostlet-core/releases/latest/download/hostlet-linux-x64
curl -fLO https://github.com/KanterLabs/hostlet-core/releases/latest/download/hostlet-linux-x64.sha256
sha256sum --check hostlet-linux-x64.sha256
sudo install -m 0755 hostlet-linux-x64 /usr/local/bin/hostlet
```

Initialize and start:

```bash
hostlet preflight
hostlet init
hostlet up
```

For self-hosted Cloudflare Tunnel mode:

```bash
hostlet up --tunnel
```

Open the URL printed by the CLI, complete first-run setup, connect GitHub, and deploy an app.

## Documentation

- [Documentation index](docs/README.md)
- [Self-hosting guide](docs/self-hosting.md)
- [Deploying apps](docs/deploying-apps.md)
- [Operations](docs/operations.md)
- [Architecture](docs/architecture.md)
- [Security](docs/security.md)

## Support And Security

Core support is best effort. For reproducible bugs and setup problems, read
[the support guide](SUPPORT.md) and open a
[GitHub issue](https://github.com/KanterLabs/hostlet-core/issues/new/choose).
Do not put credentials, environment files, private repository details, or
suspected vulnerabilities in an issue. Report security concerns through the
[security policy](SECURITY.md).

## Development

```bash
cargo run -p hostlet -- --help
docker compose -f infra/docker-compose.yml up -d
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for validation commands and release expectations.
