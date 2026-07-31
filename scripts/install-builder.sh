#!/usr/bin/env bash
set -euo pipefail

API_URL=""
ENROLLMENT_TOKEN=""
SLOTS="1"
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --api) API_URL="${2:-}"; shift 2 ;;
    --token) ENROLLMENT_TOKEN="${2:-}"; shift 2 ;;
    --slots) SLOTS="${2:-}"; shift 2 ;;
    --image) AGENT_IMAGE="${2:-}"; shift 2 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

if [[ $(id -u) -ne 0 ]]; then
  echo "run this installer with sudo" >&2
  exit 1
fi
if [[ ! "$API_URL" =~ ^https:// ]] && [[ ! "$API_URL" =~ ^http://(127\.0\.0\.1|localhost)(:|/) ]]; then
  echo "--api must use HTTPS (HTTP is allowed only for localhost)" >&2
  exit 1
fi
if [[ -z "$ENROLLMENT_TOKEN" ]]; then
  echo "--token is required" >&2
  exit 1
fi
if [[ ! "$SLOTS" =~ ^[0-9]+$ ]] || (( SLOTS < 1 || SLOTS > 16 )); then
  echo "--slots must be between 1 and 16" >&2
  exit 1
fi
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v docker >/dev/null || { echo "Docker Engine is required" >&2; exit 1; }
command -v systemctl >/dev/null || { echo "systemd is required" >&2; exit 1; }
docker info >/dev/null
docker buildx version >/dev/null
docker compose version >/dev/null

case "$(uname -m)" in
  x86_64|amd64) PLATFORM="linux/amd64" ;;
  aarch64|arm64)
    PLATFORM="linux/arm64"
    if [[ -z "$AGENT_IMAGE" ]]; then
      echo "arm64 enrollment requires --image with a pinned arm64 Hostlet agent image" >&2
      exit 1
    fi
    ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

INSTALL_DIR=/etc/hostlet-builder
STATE_DIR=/var/lib/hostlet-builder
RESPONSE_FILE=$(mktemp)
trap 'rm -f "$RESPONSE_FILE"' EXIT
HOST_NAME=$(hostname -f 2>/dev/null || hostname)
JSON=$(printf '{"enrollmentToken":"%s","name":"%s","platforms":["%s"],"maxConcurrentBuilds":%s}' \
  "$ENROLLMENT_TOKEN" "$HOST_NAME" "$PLATFORM" "$SLOTS")
HTTP_STATUS=$(curl -sS -o "$RESPONSE_FILE" -w '%{http_code}' \
  -H 'content-type: application/json' \
  --data-binary "$JSON" \
  "${API_URL%/}/api/agent/builders/register")
if [[ "$HTTP_STATUS" != "200" ]]; then
  echo "builder enrollment failed (HTTP $HTTP_STATUS)" >&2
  exit 1
fi

json_string() {
  local key="$1"
  sed -n "s/.*\"${key}\":\"\([^\"]*\)\".*/\1/p" "$RESPONSE_FILE" | head -n 1
}

SERVER_ID=$(json_string serverId)
AGENT_TOKEN=$(json_string agentToken)
SIGNING_SECRET=$(json_string jobSigningSecret)
REGISTERED_API=$(json_string apiUrl)
REGISTERED_IMAGE=$(json_string agentImage)
if [[ -z "$AGENT_IMAGE" ]]; then
  AGENT_IMAGE="$REGISTERED_IMAGE"
fi
if [[ -z "$SERVER_ID" || -z "$AGENT_TOKEN" || -z "$SIGNING_SECRET" || -z "$REGISTERED_API" ]]; then
  echo "builder enrollment returned an incomplete response" >&2
  exit 1
fi
if [[ -z "$AGENT_IMAGE" ]]; then
  echo "agent image is not configured; rerun with --image <pinned-image-ref>" >&2
  exit 1
fi
if [[ ! "$SERVER_ID" =~ ^[0-9a-fA-F-]{36}$ ]] \
  || [[ ! "$AGENT_TOKEN" =~ ^[A-Za-z0-9_-]+$ ]] \
  || [[ ! "$SIGNING_SECRET" =~ ^[A-Za-z0-9_-]+$ ]]; then
  echo "builder enrollment returned invalid credentials" >&2
  exit 1
fi
if [[ ! "$REGISTERED_API" =~ ^https:// ]] && [[ ! "$REGISTERED_API" =~ ^http://(127\.0\.0\.1|localhost)(:|/) ]]; then
  echo "builder enrollment returned an unsafe API URL" >&2
  exit 1
fi
if [[ ! "$AGENT_IMAGE" =~ ^[A-Za-z0-9._/@:-]+$ ]]; then
  echo "agent image reference contains unsafe characters" >&2
  exit 1
fi

install -d -m 0700 "$INSTALL_DIR"
install -d -m 0750 "$STATE_DIR"
umask 077
cat >"$INSTALL_DIR/agent.env" <<EOF
HOSTLET_API_URL=$REGISTERED_API
HOSTLET_SERVER_ID=$SERVER_ID
HOSTLET_AGENT_TOKEN=$AGENT_TOKEN
HOSTLET_JOB_SIGNING_SECRET=$SIGNING_SECRET
HOSTLET_WORKDIR=/var/lib/hostlet
HOSTLET_LOCAL_MODE=false
HOSTLET_APP_PUBLIC_SCHEME=https
HOSTLET_MAX_CONCURRENT_BUILDS=$SLOTS
HOSTLET_AGENT_IMAGE=$AGENT_IMAGE
EOF

DOCKER_GID=$(stat -c '%g' /var/run/docker.sock)
cat >/etc/systemd/system/hostlet-builder.service <<EOF
[Unit]
Description=Hostlet builder agent
After=docker.service network-online.target
Requires=docker.service
Wants=network-online.target

[Service]
Restart=always
RestartSec=5
EnvironmentFile=$INSTALL_DIR/agent.env
ExecStartPre=-/usr/bin/docker rm -f hostlet-builder-agent
ExecStartPre=/usr/bin/docker pull $AGENT_IMAGE
ExecStart=/usr/bin/docker run --rm --name hostlet-builder-agent --network host \\
  --group-add $DOCKER_GID --security-opt no-new-privileges:true \\
  --env-file $INSTALL_DIR/agent.env \\
  -v /var/run/docker.sock:/var/run/docker.sock \\
  -v $STATE_DIR:/var/lib/hostlet \\
  $AGENT_IMAGE
ExecStop=/usr/bin/docker stop -t 30 hostlet-builder-agent

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now hostlet-builder.service
echo "Hostlet builder enrolled as $HOST_NAME ($PLATFORM, $SLOTS slot(s))."
