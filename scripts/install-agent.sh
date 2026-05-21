#!/usr/bin/env bash
set -euo pipefail
umask 077

: "${HOSTLET_API_URL:?HOSTLET_API_URL is required}"
: "${HOSTLET_SERVER_ID:?HOSTLET_SERVER_ID is required}"
: "${HOSTLET_INSTALL_TOKEN:?HOSTLET_INSTALL_TOKEN is required}"
: "${HOSTLET_REPO_URL:?HOSTLET_REPO_URL is required. Set this to the Git repository URL for Hostlet.}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run as root with sudo." >&2
  exit 1
fi

apt-get update
apt-get install -y ca-certificates curl git jq build-essential pkg-config libssl-dev

if ! command -v docker >/dev/null 2>&1; then
  curl -fsSL https://get.docker.com | sh
fi

if ! command -v caddy >/dev/null 2>&1; then
  apt-get install -y debian-keyring debian-archive-keyring apt-transport-https
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
  apt-get update
  apt-get install -y caddy
fi

id -u hostlet >/dev/null 2>&1 || useradd --system --home /var/lib/hostlet --create-home --shell /usr/sbin/nologin hostlet
usermod -aG docker hostlet
mkdir -p /var/lib/hostlet /etc/caddy/hostlet
chown -R hostlet:hostlet /var/lib/hostlet /etc/caddy/hostlet
sudo -u hostlet bash -c 'test_file="$(mktemp /etc/caddy/hostlet/.hostlet-write-test.XXXXXX)" && rm -f "$test_file"'

if ! grep -q "import /etc/caddy/hostlet/*.caddy" /etc/caddy/Caddyfile; then
  printf '\nimport /etc/caddy/hostlet/*.caddy\n' >> /etc/caddy/Caddyfile
fi

tmp="$(mktemp -d)"
curl -fsSL https://sh.rustup.rs -o "$tmp/rustup.sh"
sudo -u hostlet bash "$tmp/rustup.sh" -y
sudo -u hostlet env HOSTLET_REPO_URL="$HOSTLET_REPO_URL" bash -lc 'cd /var/lib/hostlet && git clone "$HOSTLET_REPO_URL" src || true && cd src && git pull && ~/.cargo/bin/cargo build --release --manifest-path apps/agent/Cargo.toml'
install -m 0755 /var/lib/hostlet/src/target/release/hostlet-agent /usr/local/bin/hostlet-agent

registration="$(curl -fsSL -X POST "$HOSTLET_API_URL/api/agent/register" -H 'Content-Type: application/json' -d "{\"server_id\":\"$HOSTLET_SERVER_ID\",\"install_token\":\"$HOSTLET_INSTALL_TOKEN\"}")"
agent_token="$(printf '%s' "$registration" | jq -r '.agentToken // empty')"
job_signing_secret="$(printf '%s' "$registration" | jq -r '.jobSigningSecret // empty')"
if [[ -z "$agent_token" || -z "$job_signing_secret" ]]; then
  echo "Registration failed. Check the server token and API URL." >&2
  exit 1
fi

cat >/etc/systemd/system/hostlet-agent.service <<UNIT
[Unit]
Description=Hostlet deployment agent
After=network-online.target docker.service caddy.service
Wants=network-online.target

[Service]
User=hostlet
Group=hostlet
Environment="HOSTLET_API_URL=$HOSTLET_API_URL"
Environment="HOSTLET_SERVER_ID=$HOSTLET_SERVER_ID"
Environment="HOSTLET_AGENT_TOKEN=$agent_token"
Environment="HOSTLET_JOB_SIGNING_SECRET=$job_signing_secret"
Environment=HOSTLET_WORKDIR=/var/lib/hostlet
ExecStart=/usr/local/bin/hostlet-agent
Restart=always
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
ReadWritePaths=/var/lib/hostlet /etc/caddy/hostlet

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now hostlet-agent
systemctl reload caddy || systemctl restart caddy
echo "Hostlet agent installed."
