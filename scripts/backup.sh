#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
BACKUP_ROOT="${HOSTLET_BACKUP_ROOT:-$ROOT_DIR/backups}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
BACKUP_DIR="${1:-$BACKUP_ROOT/hostlet-$STAMP}"
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"
SCHEDULED="${HOSTLET_BACKUP_SCHEDULED:-false}"

mkdir -p "$BACKUP_DIR"

docker compose -f "$COMPOSE_FILE" exec -T postgres \
  pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB" > "$BACKUP_DIR/postgres.sql"

cat > "$BACKUP_DIR/ENVIRONMENT_REQUIRED.txt" <<'TXT'
Restore requires the same secret values used by the original deployment:

- ENCRYPTION_KEY
- SESSION_SECRET
- JOB_SIGNING_SECRET
- LOCAL_AGENT_TOKEN
- GITHUB_WEBHOOK_SECRET
- GitHub OAuth variables when GitHub login is enabled
- Cloudflare variables when public tunnels are enabled

This backup intentionally does not copy .env, because it contains live secrets.
Store your production .env in a separate password manager or secret store.
TXT

if docker volume inspect "$AGENT_VOLUME" >/dev/null 2>&1; then
  docker run --rm \
    -v "$AGENT_VOLUME:/data:ro" \
    -v "$BACKUP_DIR:/backup" \
    alpine:3.22 \
    sh -c "tar -czf /backup/hostlet-agent-state.tar.gz -C /data . && chown -R $(id -u):$(id -g) /backup"
fi

cat > "$BACKUP_DIR/manifest.txt" <<TXT
created_at=$STAMP
compose_file=$COMPOSE_FILE
postgres_db=$POSTGRES_DB
postgres_user=$POSTGRES_USER
agent_volume=$AGENT_VOLUME
scheduled=$SCHEDULED
TXT

cat > "$BACKUP_ROOT/latest.json" <<TXT
{
  "created_at": "$STAMP",
  "path": "$BACKUP_DIR",
  "compose_file": "$COMPOSE_FILE",
  "postgres_db": "$POSTGRES_DB",
  "postgres_user": "$POSTGRES_USER",
  "agent_volume": "$AGENT_VOLUME",
  "scheduled": "$SCHEDULED"
}
TXT

echo "Backup written to $BACKUP_DIR"
