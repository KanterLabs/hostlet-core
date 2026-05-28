# Hostlet

Hostlet is an open-source deployment control panel for running web apps on your own server.

## What You Get

- Self-hosted GitHub-backed app deployment.
- Dockerfile and generated Node app support.
- Deployment logs, runtime health, restart, rollback, and delete flows.
- Encrypted app environment variables.
- Optional Cloudflare Tunnel support for self-hosted public URLs.
- Image-based production releases from GHCR.

Hosted-service code, billing, private deployment config, and company infrastructure live outside this public core repo.

## Quick Start

Prerequisites:

- Linux server with Docker and Docker Compose.
- Git and curl.
- GitHub OAuth App with Device Flow enabled.

Install the self-hosted CLI:

```bash
git clone https://github.com/ShaneKanterman04/hostlet-core.git
cd hostlet-core
curl -L https://github.com/ShaneKanterman04/hostlet-core/releases/latest/download/hostlet-linux-x64 -o hostlet
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
- [Self-hosting guide](docs/self-hosting.md)
- [Deploying apps](docs/deploying-apps.md)
- [Operations](docs/operations.md)
- [Architecture](docs/architecture.md)
- [Security](docs/security.md)
- [Cloud migration notes](CLOUD_MIGRATION.md)

## Development

```bash
cargo run -p hostlet -- --help
docker compose -f infra/docker-compose.yml up -d
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for validation commands and release expectations.
