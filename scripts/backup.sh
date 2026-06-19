#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
BACKUP_ROOT="${HOSTLET_BACKUP_ROOT:-$ROOT_DIR/backups}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"
SCHEDULED="${HOSTLET_BACKUP_SCHEDULED:-false}"
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-alpine:3.22}"
# Explicit env file for compose resolution.  Standalone runs against prod need
# this because compose does not auto-load the project .env when invoked via SSH.
# Accepted via --env-file <path> flag or HOSTLET_COMPOSE_ENV_FILE env var.
COMPOSE_ENV_FILE="${HOSTLET_COMPOSE_ENV_FILE:-}"

# Parse flags; positional arg (if any) is the backup destination dir.
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
BACKUP_DIR="${BACKUP_DIR:-$BACKUP_ROOT/hostlet-$STAMP}"

# Build the docker compose base command, optionally injecting --env-file.
compose_cmd() {
  if [[ -n "$COMPOSE_ENV_FILE" ]]; then
    docker compose -f "$COMPOSE_FILE" --env-file "$COMPOSE_ENV_FILE" "$@"
  else
    docker compose -f "$COMPOSE_FILE" "$@"
  fi
}

# Emit a JSON string literal (with surrounding quotes) for an arbitrary value,
# escaping backslashes and double quotes so paths with such characters stay valid.
json_string() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

# Remove a partially written backup directory if the run fails before it is
# finalized, so failures do not leave orphaned partial dirs behind.
BACKUP_COMPLETE=false
cleanup() {
  if [[ "$BACKUP_COMPLETE" != "true" ]]; then
    rm -rf "$BACKUP_DIR"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -p "$BACKUP_DIR"

compose_cmd exec -T postgres \
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
    "$AGENT_IMAGE" \
    sh -c "tar -czf /backup/hostlet-agent-state.tar.gz -C /data . && chown -R $(id -u):$(id -g) /backup"
fi

# Single source of truth for the manifest fields, rendered into both the plain
# key=value manifest.txt and the JSON latest.json so the two cannot drift.
MANIFEST_KEYS=(created_at path compose_file postgres_db postgres_user agent_volume scheduled)
MANIFEST_VALUES=("$STAMP" "$BACKUP_DIR" "$COMPOSE_FILE" "$POSTGRES_DB" "$POSTGRES_USER" "$AGENT_VOLUME" "$SCHEDULED")

# manifest.txt mirrors latest.json minus the redundant "path" (it is the dir itself).
{
  for i in "${!MANIFEST_KEYS[@]}"; do
    [[ "${MANIFEST_KEYS[$i]}" == "path" ]] && continue
    printf '%s=%s\n' "${MANIFEST_KEYS[$i]}" "${MANIFEST_VALUES[$i]}"
  done
} > "$BACKUP_DIR/manifest.txt"

{
  printf '{\n'
  for i in "${!MANIFEST_KEYS[@]}"; do
    sep=","
    [[ "$i" -eq $((${#MANIFEST_KEYS[@]} - 1)) ]] && sep=""
    printf '  %s: %s%s\n' \
      "$(json_string "${MANIFEST_KEYS[$i]}")" \
      "$(json_string "${MANIFEST_VALUES[$i]}")" \
      "$sep"
  done
  printf '}\n'
} > "$BACKUP_ROOT/latest.json"

BACKUP_COMPLETE=true
echo "Backup written to $BACKUP_DIR"

# ---------------------------------------------------------------------------
# Off-host upload (optional).
# Set HOSTLET_BACKUP_BUCKET to a gs:// bucket path to enable off-host
# durability via gsutil rsync.  When unset this step is a no-op.
# Example: HOSTLET_BACKUP_BUCKET=gs://my-bucket/hostlet-backups
# ---------------------------------------------------------------------------
if [[ -n "${HOSTLET_BACKUP_BUCKET:-}" ]]; then
  if ! command -v gsutil >/dev/null 2>&1; then
    echo "ERROR: HOSTLET_BACKUP_BUCKET is set but gsutil is not installed/on PATH. Local backup is complete; off-host upload failed." >&2
    exit 1
  fi
  echo "Uploading backup to $HOSTLET_BACKUP_BUCKET ..."
  gsutil -m rsync -r "$BACKUP_DIR" "$HOSTLET_BACKUP_BUCKET/hostlet-$STAMP"
  echo "Off-host upload complete: $HOSTLET_BACKUP_BUCKET/hostlet-$STAMP"
else
  echo "HOSTLET_BACKUP_BUCKET not set — skipping off-host upload."
fi
