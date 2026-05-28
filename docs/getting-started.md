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
git clone https://github.com/ShaneKanterman04/Hostlet.git
cd Hostlet
curl -L https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/hostlet-linux-x64 -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

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

Then open the URL printed by the CLI, enter the setup token if prompted, set the control-plane password, unlock the panel, and connect GitHub.

## First App

1. Click **New app**.
2. Pick a GitHub repository.
3. Choose the detected app shape.
4. Click **Deploy**.
5. Watch logs until the deployment succeeds.

See [Deploying Apps](deploying-apps.md) for supported app shapes and limits.

## Useful Commands

```bash
hostlet status
hostlet logs
hostlet doctor
hostlet update check
hostlet update
hostlet backup
hostlet down
```
