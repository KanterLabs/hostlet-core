# Hostlet

Hostlet is an open-source deployment control panel for running web apps on your own server.

## What You Get

- Self-hosted GitHub-backed app deployment.
- Dockerfile and Railpack generated app support.
- Deployment logs, runtime health, restart, rollback, and delete flows.
- Encrypted app environment variables.
- App screenshot capture for exposed app URLs.
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
ARCH="$(uname -m)"; case "$ARCH" in x86_64) ASSET=hostlet-linux-x64;; aarch64|arm64) ASSET=hostlet-linux-arm64;; *) echo "Unsupported architecture: $ARCH" >&2; exit 1;; esac
curl -L "https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/$ASSET" -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

The public source lives in `hostlet-core`; release assets are currently
published from the `ShaneKanterman04/Hostlet` GitHub Releases feed that the CLI
updater reads.

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

## Development

```bash
cargo run -p hostlet -- --help
docker compose -f infra/docker-compose.yml up -d
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for validation commands and release expectations.
