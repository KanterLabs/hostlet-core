<h1 align="center">
  <img src="https://readme-typing-svg.demolab.com?font=Fira+Code&size=32&duration=3000&pause=1000&color=2D9CDB&center=true&vCenter=true&width=435&lines=Hostlet" alt="Hostlet" />
</h1>

<p align="center">
  <img src="https://readme-typing-svg.demolab.com?font=Fira+Code&size=18&duration=3000&pause=1000&color=27AE60&center=true&vCenter=true&width=500&lines=Self-hosted+deployment+that+just+works+%F0%9F%9A%80;Deploy+GitHub+projects+in+30+seconds+%E2%9A%A1;Your+apps%2C+your+server%2C+your+rules+%F0%9F%94%92" alt="Tagline" />
</p>

<p align="center">
  <a href="https://github.com/ShaneKanterman04/Hostlet/releases/latest">
    <img src="https://img.shields.io/github/v/release/ShaneKanterman04/Hostlet?color=2D9CDB&style=for-the-badge&logo=github" alt="Latest Release" />
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-27AE60?style=for-the-badge" alt="MIT License" />
  </a>
  <a href="https://www.rust-lang.org/">
    <img src="https://img.shields.io/badge/built%20with-Rust-orange?style=for-the-badge&logo=rust&logoColor=white" alt="Built with Rust" />
  </a>
  <a href="https://nextjs.org/">
    <img src="https://img.shields.io/badge/frontend-Next.js-black?style=for-the-badge&logo=next.js&logoColor=white" alt="Next.js" />
  </a>
  <a href="https://www.docker.com/">
    <img src="https://img.shields.io/badge/dockerized-2496ED?style=for-the-badge&logo=docker&logoColor=white" alt="Docker" />
  </a>
</p>

---

<p align="center">
  <b>Hostlet</b> is your personal deployment control panel. <br/>
  Push code to GitHub → watch it deploy. No cloud vendor lock-in. No monthly fees. Just you and your server.
</p>

## ✨ What You Get

| Feature | Status |
|---------|--------|
| 🖥️ Web Dashboard (Next.js) | ✅ Ready |
| 🦀 Rust API Control Plane | ✅ Ready |
| 🐳 Docker-based Deployments | ✅ Ready |
| 🌐 Cloudflare Tunnel Support | ✅ Ready |
| 🔄 Auto-redeploy on Git Push | ✅ Ready |
| 🔐 GitHub OAuth Device Flow | ✅ Ready |
| 📊 Live Deployment Logs | ✅ Ready |
| 🗄️ PostgreSQL Database | ✅ Ready |
| 🛡️ Caddy Reverse Proxy | ✅ Ready |
| 💾 Backup & Restore Scripts | ✅ Ready |

> Hostlet 0.1.0 is local-machine-only: the UI/API and deployed app containers run on the same host. Remote VPS agents are deferred.

## 🚀 Quick Start

### Prerequisites

- Docker & Docker Compose
- Git & curl
- A GitHub OAuth App with Device Flow enabled
- *(Optional)* Cloudflare zone + tunnel token for public `*.your-domain.com` URLs

### 1. Install

```bash
git clone https://github.com/ShaneKanterman04/Hostlet.git
cd Hostlet
curl -L https://github.com/ShaneKanterman04/Hostlet/releases/latest/download/hostlet-linux-x64 -o hostlet
chmod +x hostlet
sudo mv hostlet /usr/local/bin/hostlet
```

### 2. Run the Wizard

```bash
hostlet init
```

This generates your `.env`, asks for your GitHub OAuth Client ID, optionally configures Cloudflare Tunnel, and prints your first setup token.

### 3. Pick Your Mode

| Mode | URL | Webhooks? |
|------|-----|-----------|
| **LAN Only** | `http://YOUR_HOST_IP:3000` | Manual deploys only |
| **Cloudflare Tunnel** | `https://your-domain.com` | Auto-redeploy from GitHub |

> 💡 **Pro tip:** Deployed apps stay **private by default** in both modes. You choose which ones go public.

### 4. Create a GitHub OAuth App

Enable **Device Flow** on your OAuth App and copy the Client ID into Hostlet.

For LAN testing:
```
Homepage URL: http://YOUR_HOST_IP:3000
```

For Cloudflare Tunnel mode:
```
Homepage URL: https://hostlet.example.com
```

> 🔑 Hostlet uses Device Flow — no callback URL or client secret needed!

### 5. Start Hostlet

```bash
# LAN mode
hostlet up

# Cloudflare Tunnel mode
hostlet up --tunnel
```

Open the UI URL printed by `hostlet init` and you're live! 🎉

---

## 🎯 First-Time Setup

1. Paste your setup token (from `hostlet init`)
2. Set your first-run password
3. Unlock the panel
4. Sign in with GitHub
5. Create an app → Deploy to **This machine**

In LAN mode, deploy manually:
1. Push changes to GitHub
2. Open the app in Hostlet
3. Click **Deploy latest** 🚀

For automatic deploys, use Cloudflare Tunnel mode or set `PUBLIC_WEBHOOK_URL`.

---

## 🌐 Public App URLs (Optional)

Want public-facing app URLs? Add these to `.env`:

```bash
HOSTLET_BASE_DOMAIN=example.com
HOSTLET_DOMAIN_PREFIX=hostlet-
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ZONE_ID=...
CLOUDFLARE_TUNNEL_TARGET=<tunnel-id>.cfargotunnel.com
CLOUDFLARE_TUNNEL_TOKEN=...
```

Toggle **Publish URL** or **Make private** on any app detail page. You're in control.

---

## 🔄 Auto-Redeploy from GitHub

Auto-redeploy is off by default and requires a public webhook endpoint.

**Full tunnel mode:**
```
PUBLIC_API_URL=https://hostlet.example.com
```

**LAN UI + public webhook:**
```
PUBLIC_API_URL=http://YOUR_LAN_IP:8080
PUBLIC_WEBHOOK_URL=https://hostlet.example.com
```

### Enable auto-redeploy:

1. Run `hostlet up --tunnel` (or set `PUBLIC_WEBHOOK_URL`)
2. Enable **Auto redeploy on branch push** for your app
3. Hostlet creates or updates the GitHub webhook using your connected GitHub token:
   ```
   Payload URL: PUBLIC_WEBHOOK_URL/webhooks/github
   Content type: application/json
   Secret: GITHUB_WEBHOOK_SECRET
   Events: push
   ```

Only matching repo/branch pushes trigger deployments, and only for apps where auto-redeploy is enabled.

---

## 💾 Backup & Restore

```bash
# Create backup
scripts/backup.sh

# Restore from backup
HOSTLET_RESTORE_CONFIRM=yes scripts/restore.sh backups/hostlet-YYYYMMDDTHHMMSSZ
```

> 🔐 Keep `.env` secrets in a password manager. Backups store a checklist, not live secrets.

---

## 🏗️ Architecture

```
Browser
  |
  | HTTP :3000
  v
Next.js Web UI
  |
  | HTTP API / WebSocket Logs :8080
  v
Rust API Control Plane
  |
  | PostgreSQL
  v
Postgres DB

Rust API <-- WebSocket/Events --> Hostlet Agent
                                    |
                                    | Docker Socket
                                    v
                              App Containers

Cloudflare Edge -> cloudflared -> Caddy -> App Container
```

- **Web** (`apps/web`): Next.js dashboard with live logs
- **API** (`apps/api`): Axum control plane with auth & deployments
- **Agent** (`apps/agent`): Deployment executor with Docker
- **CLI** (`apps/cli`): Setup wizard & management commands

---

## 🛠️ Development

```bash
# Run from source (CLI)
cargo run -p hostlet -- <command>

# Start all services for development
pnpm dev

# Or manually
docker compose -f infra/docker-compose.yml up --build
```

### Production Deploy

```bash
# Build production images (no source bind mounts)
docker compose -f infra/docker-compose.prod.yml up -d --build

# With Cloudflare Tunnel
docker compose -f infra/docker-compose.prod.yml --profile tunnel up -d --build
```

---

## 📚 Documentation

- 📖 [Full Guide](docs/README.md)
- 🏗️ [Architecture](docs/ARCHITECTURE.md)
- 🔒 [Security](docs/SECURITY.md)
- 📝 [Feature Gaps](docs/FEATURE_GAPS.md)

---

<p align="center">
  <img src="https://readme-typing-svg.demolab.com?font=Fira+Code&size=14&duration=4000&pause=1000&color=888888&center=true&vCenter=true&width=500&lines=Made+with+%E2%9D%A4%EF%B8%8F+by+Shane+Kanterman;Star+%E2%AD%90+this+repo+if+it+helped+you!" alt="Footer" />
</p>

<p align="center">
  <a href="https://github.com/ShaneKanterman04/Hostlet/stargazers">
    <img src="https://img.shields.io/github/stars/ShaneKanterman04/Hostlet?style=social" alt="GitHub Stars" />
  </a>
</p>
