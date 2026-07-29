# Getting Started

This guide gets Hostlet Core running on one Linux server with Docker.

## Prerequisites

- Linux x86_64 with glibc 2.39 or newer. Ubuntu 24.04 LTS and newer meet this
  baseline. Alpine/musl and distributions with older glibc versions do not;
  ARM64 release artifacts are not published.
- Docker Engine with a running daemon and the Docker Compose v2 plugin. The
  installing user must be able to run Docker commands.
- Git and curl.
- A GitHub OAuth App with Device Flow enabled.
- A GitHub account that will own the Hostlet install.
- More than 1 GiB of free disk space, which `hostlet preflight` enforces. App
  builds and images normally need substantially more; Core does not publish or
  enforce a fixed CPU, RAM, or production-capacity minimum.

[Hostlet Cloud](https://hostlet.cloud) is the managed portfolio service; it is
not required to install or run Hostlet Core.

## Install

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
hostlet version
```

Run the preflight before initialization. It checks the host OS, x86_64
architecture, glibc baseline, reachable Docker daemon, Compose v2, and free
disk space:

```bash
hostlet preflight
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

Then open the URL printed by the CLI, enter the setup token if prompted, set a
control-plane password of at least 12 characters, unlock the panel, and connect
GitHub. The setup token field is used only when the install was configured with
one.

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
