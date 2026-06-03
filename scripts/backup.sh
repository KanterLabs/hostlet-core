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
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-alpine:3.22}"

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
