#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
BACKUP_ROOT="${HOSTLET_BACKUP_ROOT:-$ROOT_DIR/backups}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
POSTGRES_USER="${POSTGRES_USER:-}"
POSTGRES_DB="${POSTGRES_DB:-}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"
SCREENSHOT_VOLUME="${HOSTLET_SCREENSHOT_VOLUME:-infra_hostlet-screenshots}"
SCHEDULED="${HOSTLET_BACKUP_SCHEDULED:-false}"
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-alpine:3.22}"
# Explicit env file for compose resolution.  Standalone runs against prod need
# this because compose does not auto-load the project .env when invoked via SSH.
# Accepted via --env-file <path> flag or HOSTLET_COMPOSE_ENV_FILE env var.
COMPOSE_ENV_FILE="${HOSTLET_COMPOSE_ENV_FILE:-}"

if [[ "$BACKUP_ROOT" != /* ]]; then
  BACKUP_ROOT="$PWD/$BACKUP_ROOT"
fi
mkdir -p -- "$BACKUP_ROOT"
BACKUP_ROOT="$(realpath -e -- "$BACKUP_ROOT")"

# Parse flags; positional arg (if any) is the backup destination dir.
REQUESTED_BACKUP_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file)
      [[ $# -ge 2 ]] || { echo "--env-file requires a path" >&2; exit 2; }
      COMPOSE_ENV_FILE="$2"
      shift 2
      ;;
    --env-file=*)
      COMPOSE_ENV_FILE="${1#*=}"
      shift
      ;;
    *)
      [[ -z "$REQUESTED_BACKUP_DIR" ]] || { echo "backup accepts exactly one destination" >&2; exit 2; }
      REQUESTED_BACKUP_DIR="$1"
      shift
      ;;
  esac
done

read_env_value() {
  local key="$1" file="$2" line value
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^[[:space:]]*${key}[[:space:]]*=(.*)$ ]] || continue
    value="${BASH_REMATCH[1]}"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    if [[ "$value" == \"* && "$value" == *\" ]]; then
      value="${value:1:${#value}-2}"
      value="${value//\\\"/\"}"
      value="${value//\\\\/\\}"
    elif [[ "$value" == \'* && "$value" == *\' ]]; then
      value="${value:1:${#value}-2}"
    fi
    printf '%s' "$value"
    return 0
  done < "$file"
  return 1
}

if [[ -n "$COMPOSE_ENV_FILE" ]]; then
  [[ -f "$COMPOSE_ENV_FILE" && ! -L "$COMPOSE_ENV_FILE" ]] || {
    echo "Compose env file is missing or is not a regular file: $COMPOSE_ENV_FILE" >&2
    exit 1
  }
  if [[ "$COMPOSE_ENV_FILE" != /* ]]; then
    COMPOSE_ENV_FILE="$PWD/$COMPOSE_ENV_FILE"
  fi
  selected_user="$(read_env_value POSTGRES_USER "$COMPOSE_ENV_FILE" || true)"
  selected_db="$(read_env_value POSTGRES_DB "$COMPOSE_ENV_FILE" || true)"
  POSTGRES_USER="${selected_user:-$POSTGRES_USER}"
  POSTGRES_DB="${selected_db:-$POSTGRES_DB}"
fi
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"

# The destination is never used as the write target.  A run writes to a
# private sibling and promotes it only after every artifact has been written.
# This keeps a failed backup from deleting or overwriting a caller-owned path.
REQUESTED_BACKUP_DIR="${REQUESTED_BACKUP_DIR:-$BACKUP_ROOT/hostlet-$STAMP}"
if [[ "$REQUESTED_BACKUP_DIR" != /* ]]; then
  REQUESTED_BACKUP_DIR="$PWD/$REQUESTED_BACKUP_DIR"
fi
BACKUP_PARENT="$(realpath -m -- "$(dirname -- "$REQUESTED_BACKUP_DIR")")"
mkdir -p -- "$BACKUP_PARENT"
BACKUP_PARENT="$(realpath -e -- "$BACKUP_PARENT")"
FINAL_BACKUP_DIR="$BACKUP_PARENT/$(basename -- "$REQUESTED_BACKUP_DIR")"
[[ "$FINAL_BACKUP_DIR" != "/" && "$FINAL_BACKUP_DIR" != "$ROOT_DIR" ]] || {
  echo "refusing dangerously broad backup destination: $FINAL_BACKUP_DIR" >&2
  exit 1
}
[[ ! -e "$FINAL_BACKUP_DIR" && ! -L "$FINAL_BACKUP_DIR" ]] || {
  echo "backup destination already exists: $FINAL_BACKUP_DIR" >&2
  exit 1
}
RUN_TOKEN="$$-${RANDOM}-${RANDOM}"
BACKUP_DIR=""
BACKUP_COMPLETE=false
LATEST_TEMP=""

cleanup() {
  local status=$?
  trap - EXIT
  # Only remove a directory created by this invocation and carrying its exact
  # ownership marker.  Never recursively remove the requested final path.
  if [[ -n "$BACKUP_DIR" && "$BACKUP_COMPLETE" != "true" && "$BACKUP_DIR" == "$BACKUP_PARENT"/.hostlet-backup-tmp.* && -d "$BACKUP_DIR" && ! -L "$BACKUP_DIR" ]]; then
    if [[ "$(cat -- "$BACKUP_DIR/.hostlet-backup-owned" 2>/dev/null)" == "hostlet-backup:$RUN_TOKEN" ]]; then
      rm -rf -- "$BACKUP_DIR"
    fi
  fi
  if [[ -n "$LATEST_TEMP" && "$LATEST_TEMP" == "$BACKUP_ROOT"/.hostlet-latest.* && -f "$LATEST_TEMP" && ! -L "$LATEST_TEMP" ]]; then
    rm -f -- "$LATEST_TEMP"
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

BACKUP_DIR="$(mktemp -d --tmpdir="$BACKUP_PARENT" .hostlet-backup-tmp.XXXXXX)"
chmod 700 -- "$BACKUP_DIR"
printf 'hostlet-backup:%s\n' "$RUN_TOKEN" > "$BACKUP_DIR/.hostlet-backup-owned"
chmod 600 -- "$BACKUP_DIR/.hostlet-backup-owned"

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

test_hook() {
  [[ "${HOSTLET_BACKUP_TEST_FAIL_AT:-}" == "$1" ]] || return 0
  echo "backup self-test injected failure at $1" >&2
  return 97
}

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

archive_volume() {
  local volume="$1" archive="$2"
  if docker volume inspect "$volume" >/dev/null 2>&1; then
    docker run --rm \
      -v "$volume:/data:ro" \
      -v "$BACKUP_DIR:/backup" \
      "$AGENT_IMAGE" \
      sh -c "tar -czf /backup/$archive -C /data . && chown -R $(id -u):$(id -g) /backup"
  fi
}

# Include the two persistent Hostlet state volumes. The screenshot volume is
# intentionally separate from the agent volume because production Compose
# mounts it into the API container at /var/lib/hostlet/screenshots.
archive_volume "$AGENT_VOLUME" hostlet-agent-state.tar.gz
archive_volume "$SCREENSHOT_VOLUME" hostlet-screenshots-state.tar.gz

# Single source of truth for the manifest fields, rendered into both the plain
# key=value manifest.txt and the JSON latest.json so the two cannot drift.
MANIFEST_KEYS=(created_at path compose_file postgres_db postgres_user agent_volume screenshot_volume scheduled)
MANIFEST_VALUES=("$STAMP" "$FINAL_BACKUP_DIR" "$COMPOSE_FILE" "$POSTGRES_DB" "$POSTGRES_USER" "$AGENT_VOLUME" "$SCREENSHOT_VOLUME" "$SCHEDULED")

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
} > "$BACKUP_DIR/latest.json"

# The marker is implementation detail and must not escape the staging area.
rm -f -- "$BACKUP_DIR/.hostlet-backup-owned"
mv -T -- "$BACKUP_DIR" "$FINAL_BACKUP_DIR"
BACKUP_DIR="$FINAL_BACKUP_DIR"
BACKUP_COMPLETE=true

# Update the pointer only after the final directory is visible.  A temporary
# sibling avoids exposing a half-written JSON document to readers.
LATEST_TEMP="$(mktemp --tmpdir="$BACKUP_ROOT" .hostlet-latest.XXXXXX)"
chmod 600 -- "$LATEST_TEMP"
cp -- "$BACKUP_DIR/latest.json" "$LATEST_TEMP"
test_hook metadata-write
mv -T -- "$LATEST_TEMP" "$BACKUP_ROOT/latest.json"

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

# ---------------------------------------------------------------------------
# Off-host upload to an S3-compatible bucket (optional, independent of the
# gsutil hook above — set at most one of the two).
# Set HOSTLET_BACKUP_S3_BUCKET to an s3://bucket[/prefix] path to enable
# off-host durability via `aws s3 sync` against any S3-compatible endpoint
# (AWS S3, Cloudflare R2, MinIO, ...). When unset this step is a no-op.
# Credentials/region come from the standard AWS CLI environment variables
# (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_DEFAULT_REGION) — this script
# does not read, generate, or store them itself.
# Set HOSTLET_BACKUP_S3_ENDPOINT for a non-AWS endpoint, e.g. Cloudflare R2:
#   HOSTLET_BACKUP_S3_ENDPOINT=https://<account_id>.r2.cloudflarestorage.com
# Example: HOSTLET_BACKUP_S3_BUCKET=s3://my-bucket/hostlet-backups
# ---------------------------------------------------------------------------
if [[ -n "${HOSTLET_BACKUP_S3_BUCKET:-}" ]]; then
  if ! command -v aws >/dev/null 2>&1; then
    echo "ERROR: HOSTLET_BACKUP_S3_BUCKET is set but the aws CLI is not installed/on PATH. Local backup is complete; off-host upload failed." >&2
    exit 1
  fi
  echo "Uploading backup to $HOSTLET_BACKUP_S3_BUCKET/hostlet-$STAMP ..."
  if [[ -n "${HOSTLET_BACKUP_S3_ENDPOINT:-}" ]]; then
    aws s3 sync --endpoint-url "$HOSTLET_BACKUP_S3_ENDPOINT" "$BACKUP_DIR" "$HOSTLET_BACKUP_S3_BUCKET/hostlet-$STAMP"
  else
    aws s3 sync "$BACKUP_DIR" "$HOSTLET_BACKUP_S3_BUCKET/hostlet-$STAMP"
  fi
  echo "Off-host S3 upload complete: $HOSTLET_BACKUP_S3_BUCKET/hostlet-$STAMP"
else
  echo "HOSTLET_BACKUP_S3_BUCKET not set — skipping off-host S3 upload."
fi
