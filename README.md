<h1 align="center">
  <img src="https://readme-typing-svg.demolab.com?font=Fira+Code&size=32&duration=3000&pause=1000&color=2D9CDB&center=true&vCenter=true&width=435&lines=Hostlet" alt="Hostlet" />
</h1>

<p align="center">
  <b>Self-hosted deploys for your server. Private beta managed deploys on Hostlet Cloud.</b>
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
  <b>Hostlet</b> is a deployment control panel for self-hosted servers and the private Hostlet Cloud beta. <br/>
  Push code to GitHub, watch it build, stream logs, and run behind Caddy.
</p>

## ✨ What You Get

| Feature | Status |
|---------|--------|
| 🖥️ Web Dashboard (Next.js) | ✅ Ready |
| 🦀 Rust API Control Plane | ✅ Ready |
| 🐳 Docker-based Deployments | ✅ Ready |
| 🧩 Docker Compose Multi-service Apps | ✅ Ready |
| 🌐 Cloudflare Tunnel Support | ✅ Ready |
| 🔄 Auto-redeploy on Git Push | ✅ Ready |
| 🔐 GitHub OAuth Device Flow | ✅ Ready |
| 📊 Live Deployment Logs | ✅ Ready |
| ❤️ Runtime Health Checks | ✅ Ready |
| ⬆️ Update Detection + CLI Update | ✅ Ready |
| 🗄️ PostgreSQL Database | ✅ Ready |
| 🛡️ Caddy Reverse Proxy | ✅ Ready |
| 💾 Backup & Restore Scripts | ✅ Ready |
| ☁️ Hostlet Cloud Private Beta | 🧪 0.4.0 beta |
| 💳 Stripe Sandbox Billing | 🧪 0.4.0 beta |
| 🔐 GitHub App Cloud Auth | 🧪 0.4.0 beta |

> Self-hosted Hostlet remains single-machine for 0.4.0: the UI/API and deployed app containers run on the same host. Hostlet Cloud is a separate private beta at `hostlet.cloud` using managed worker compute and `*.hostlet.cloud` app URLs. Remote self-hosted VPS agents remain deferred.

## Supported App Shapes

- **Dockerfile apps:** any language or framework that builds a container and serves HTTP on the configured port.
- **Generated Node apps:** package.json projects using npm, pnpm, or yarn, including Next.js, Vite, Astro, Nuxt, Remix, SvelteKit, and generic Node.
- **Compose apps:** one public web service plus private supporting services such as workers, Redis, Postgres, queues, or sidecars. Compose apps use a constrained `compose.yaml` plus `hostlet.yml`.

Compose apps expose one public route. Named volumes persist across redeploys. Compose rollback is disabled and clearly labeled as unsupported in 0.4.0; redeploy the desired revision instead. Hostlet does not roll database or volume contents back.

## 🚀 Quick Start

### Prerequisites

- Docker & Docker Compose
- Git & curl
- A GitHub OAuth App with Device Flow enabled
- *(Optional)* Cloudflare zone + tunnel token for public `*.your-domain.com` URLs

### 1. Install Self-Hosted Hostlet

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

## Hostlet Cloud Private Beta

Hostlet Cloud is the hosted 0.4.0 beta path at `https://hostlet.cloud`.

- Cloud apps deploy to managed Hostlet workers and receive generated `*.hostlet.cloud` URLs.
- Cloud sign-in uses GitHub OAuth plus GitHub App installation for repository access.
- Billing uses Stripe sandbox in 0.4.0. Checkout alone is not authoritative; Stripe subscription webhooks must mark the subscription active or trialing before compute is available.
- Cloud app creation and runtime mutations require an active cloud session, GitHub App installation, and active subscription.
- Cloud does not support custom domains, Compose apps, public/private toggles, auto-redeploy toggles, arbitrary CPU/RAM edits, managed databases, persistent disk upsells, multi-worker scheduling, or Stripe live mode in 0.4.0.
- Cloud secrets such as Stripe keys, GitHub App private keys, Cloudflare tokens, worker tokens, and queue access stay on the cloud VM and are never passed to customer apps.

Self-hosted installs do not require a Hostlet Cloud account, Stripe subscription, or GitHub App installation. They keep Device Flow login, local deploys, Cloudflare Tunnel, webhooks, publish/private controls, rollback for single-service apps, restart, and delete.

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

## Runtime Health and Updates

Hostlet keeps checking deployed apps after deployment. The dashboard shows runtime health separately from deployment status, with **Check now** and **Restart container** actions on each app detail page.

Keep Hostlet itself current from the server:

```bash
hostlet update check
hostlet update --dry-run
hostlet update
```

The Settings page also shows update availability and links to release notes.

---

## 🌐 Self-Hosted Public App URLs (Optional)

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

Raw Docker-published app ports bind to loopback in 0.4.0. Public exposure should go through Caddy and Cloudflare Tunnel or another trusted reverse proxy, not direct Docker host ports.

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

Self-hosted public path:
Cloudflare Edge -> cloudflared -> Caddy -> App loopback port

Hostlet Cloud beta path:
Cloudflare Edge -> hostlet.cloud Caddy -> Web/API or managed app loopback port
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
# Pull prebuilt production images and restart without building on the VM
scripts/deploy-hostlet-cloud-images.sh

# Optional release rollback/deploy: set HOSTLET_IMAGE_TAG=vX.Y.Z in .env first
scripts/deploy-hostlet-cloud-images.sh

# With Cloudflare Tunnel
docker compose --env-file .env -f infra/docker-compose.prod.yml --profile tunnel up -d --no-build
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
