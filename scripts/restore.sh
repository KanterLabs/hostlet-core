#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"

BACKUP_DIR="${1:-}"
if [[ -z "$BACKUP_DIR" || ! -f "$BACKUP_DIR/postgres.sql" ]]; then
  echo "Usage: $0 /path/to/hostlet-backup" >&2
  exit 1
fi

if [[ "${HOSTLET_RESTORE_CONFIRM:-}" != "yes" ]]; then
  echo "Refusing to restore without HOSTLET_RESTORE_CONFIRM=yes." >&2
  echo "This replaces the current Hostlet database contents." >&2
  exit 1
fi

docker compose -f "$COMPOSE_FILE" exec -T postgres \
  psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
  -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"

docker compose -f "$COMPOSE_FILE" exec -T postgres \
  psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" < "$BACKUP_DIR/postgres.sql"

if [[ -f "$BACKUP_DIR/hostlet-agent-state.tar.gz" ]]; then
  docker volume create "$AGENT_VOLUME" >/dev/null
  docker run --rm \
    -v "$AGENT_VOLUME:/data" \
    -v "$BACKUP_DIR:/backup:ro" \
    alpine:3.22 \
    sh -lc 'rm -rf /data/* && tar -xzf /backup/hostlet-agent-state.tar.gz -C /data'
fi

echo "Restore complete."
