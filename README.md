# Hostlet

Hostlet is an open-source deployment control panel for running web apps on your own server. Hostlet Cloud is the managed SaaS app-hosting service built from the same codebase.

## Hostlet vs Hostlet Cloud

- **Hostlet**: self-hosted, open source, single-machine app hosting on your Docker server. You own the machine, credentials, app data, and network exposure.
- **Hostlet Cloud**: managed hosting at `hostlet.cloud`. Hostlet operates the infrastructure, billing, provider credentials, and managed compute.

Both ship from the same `main` branch and tagged releases. Runtime mode controls the product behavior: `HOSTLET_MODE=self_hosted` for self-hosted installs and `HOSTLET_MODE=cloud` for Hostlet Cloud.

## What You Get

- GitHub-backed app deployment.
- Dockerfile and generated Node app support.
- Deployment logs, runtime health, restart, rollback, and delete flows.
- Encrypted app environment variables.
- Optional Cloudflare Tunnel support for self-hosted public URLs.
- Image-based production releases from GHCR.

Hostlet Cloud adds managed app compute, `*.hostlet.cloud` app URLs, GitHub App repository access, and billing gates. Hostlet Cloud customer apps never receive platform secrets such as worker tokens, Cloudflare tokens, Stripe secrets, GitHub App private keys, direct database access, or direct job-queue access.

## Quick Start

Prerequisites:

- Linux server with Docker and Docker Compose.
- Git and curl.
- GitHub OAuth App with Device Flow enabled.

Install the self-hosted CLI:

```bash
git clone https://github.com/ShaneKanterman04/Hostlet.git
cd Hostlet
curl -L https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/hostlet-linux-x64 -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

Initialize and start:

```bash
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
- [Hostlet vs Hostlet Cloud](docs/hostlet-vs-hostlet-cloud.md)
- [Self-hosting guide](docs/self-hosting.md)
- [Hostlet Cloud guide](docs/hostlet-cloud.md)
- [Deploying apps](docs/deploying-apps.md)
- [Operations](docs/operations.md)
- [Architecture](docs/architecture.md)
- [Security](docs/security.md)

## Development

```bash
cargo run -p hostlet -- --help
docker compose -f infra/docker-compose.yml up -d
```

See [contributing](docs/contributing.md) for validation commands and release expectations.
