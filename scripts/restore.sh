#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-alpine:3.22}"

# Run psql against the running postgres service over docker compose exec.
# Extra args (e.g. -c "...") are forwarded; stdin is passed through so callers
# can pipe a SQL dump in.
psql_exec() {
  docker compose -f "$COMPOSE_FILE" exec -T postgres \
    psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" "$@"
}

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

# After the schema is dropped the database is empty until the dump finishes
# loading. Warn loudly if the restore step fails midway so the empty-DB state
# is not silently mistaken for success.
RESTORE_OK=false
warn_partial_restore() {
  if [[ "$RESTORE_OK" != "true" ]]; then
    echo "Restore failed after dropping the schema; the database may be empty." >&2
    echo "Re-run this script with the same backup to retry the restore." >&2
  fi
}
trap warn_partial_restore EXIT

psql_exec -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"

psql_exec < "$BACKUP_DIR/postgres.sql"

RESTORE_OK=true

if [[ -f "$BACKUP_DIR/hostlet-agent-state.tar.gz" ]]; then
  docker volume create "$AGENT_VOLUME" >/dev/null
  docker run --rm \
    -v "$AGENT_VOLUME:/data" \
    -v "$BACKUP_DIR:/backup:ro" \
    "$AGENT_IMAGE" \
    sh -lc 'rm -rf /data/* && tar -xzf /backup/hostlet-agent-state.tar.gz -C /data'
fi

echo "Restore complete."
