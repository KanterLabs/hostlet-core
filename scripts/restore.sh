#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-alpine:3.22}"
# Explicit env file for compose resolution.  Standalone runs against prod need
# this because compose does not auto-load the project .env when invoked via SSH.
# Accepted via --env-file <path> flag or HOSTLET_COMPOSE_ENV_FILE env var.
COMPOSE_ENV_FILE="${HOSTLET_COMPOSE_ENV_FILE:-}"

# Run psql against the running postgres service over docker compose exec,
# optionally injecting --env-file so compose can resolve required secrets.
# Extra args (e.g. -c "...") are forwarded; stdin is passed through so callers
# can pipe a SQL dump in.
psql_exec() {
  if [[ -n "$COMPOSE_ENV_FILE" ]]; then
    docker compose -f "$COMPOSE_FILE" --env-file "$COMPOSE_ENV_FILE" exec -T postgres \
      psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" "$@"
  else
    docker compose -f "$COMPOSE_FILE" exec -T postgres \
      psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" "$@"
  fi
}

# Parse flags; positional arg (if any) is the backup source dir.
BACKUP_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file)
      COMPOSE_ENV_FILE="$2"
      shift 2
      ;;
    --env-file=*)
      COMPOSE_ENV_FILE="${1#*=}"
      shift
      ;;
    *)
      BACKUP_DIR="$1"
      shift
      ;;
  esac
done
if [[ -z "$BACKUP_DIR" ]]; then
  echo "Usage: $0 [--env-file <path>] /path/to/hostlet-backup" >&2
  exit 1
fi
# Reject missing or zero-byte dumps before any destructive step.  A power-loss
# during backup.sh can leave a 0-byte postgres.sql that would otherwise destroy
# the live DB and print "Restore complete." using `-f` alone.
if [[ ! -s "$BACKUP_DIR/postgres.sql" ]]; then
  echo "Backup file $BACKUP_DIR/postgres.sql is missing or empty; refusing to restore." >&2
  exit 1
fi
# Verify the dump carries the pg_dump header so a truncated or wrong-format
# file is caught before the schema is dropped.
if ! head -c 512 "$BACKUP_DIR/postgres.sql" | grep -q 'PostgreSQL database dump'; then
  echo "$BACKUP_DIR/postgres.sql does not look like a pg_dump SQL backup; refusing to restore." >&2
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

psql_exec -v ON_ERROR_STOP=1 --single-transaction < "$BACKUP_DIR/postgres.sql"

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
