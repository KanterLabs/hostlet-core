# Getting Started

This guide gets self-hosted Hostlet running on a Linux server with Docker.

## Prerequisites

- Docker and Docker Compose.
- Git and curl.
- A GitHub OAuth App with Device Flow enabled.
- A GitHub account that will own the Hostlet install.

The hosted-service layer is not required for self-hosted use.

## Install

```bash
git clone https://github.com/ShaneKanterman04/hostlet-core.git
cd hostlet-core
curl -L https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/hostlet-linux-x64 -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

The public source lives in `hostlet-core`; release assets are currently
published from the `ShaneKanterman04/Hostlet` GitHub Releases feed that the CLI
updater reads.

## Initialize

```bash
hostlet init
```

The wizard writes `.env`, generates required secrets, asks for your GitHub OAuth Client ID, configures access mode, and prints the first setup token.

For self-hosted installs, GitHub uses Device Flow. You do not need a redirect URI or OAuth client secret.

## Start

LAN-only mode:

```bash
hostlet up
```

Cloudflare Tunnel mode:

```bash
hostlet up --tunnel
```

Then open the URL printed by the CLI, enter the setup token if prompted, set a
control-plane password of at least 12 characters, unlock the panel, and connect
GitHub. The setup token field is used only when the install was configured with
one.

The web UI includes a persisted light/dark/system theme toggle. It is shown in
the side rail on desktop and in the top-right corner on mobile, and it applies
before the page paints on later visits.

## First App

1. Click **New app**.
2. Choose a GitHub repository or paste a repo URL.
3. Click **Inspect repo**.
4. Review the inferred runtime, environment keys, route, and deploy settings.
5. Click **Create and deploy**.
6. Hostlet opens deployment logs when a deployment starts; otherwise it opens the app detail page.

See [Deploying Apps](deploying-apps.md) for supported app shapes and limits.

## Common Commands

```bash
hostlet version
hostlet status
hostlet logs
hostlet doctor
hostlet update check
hostlet update --dry-run
hostlet update
hostlet update rollback
hostlet backup
hostlet backup --scheduled
hostlet cleanup --dry-run
hostlet cleanup --yes
hostlet down
```
