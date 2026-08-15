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

compose_cmd() {
  if [[ -n "$COMPOSE_ENV_FILE" ]]; then
    docker compose -f "$COMPOSE_FILE" --env-file "$COMPOSE_ENV_FILE" "$@"
  else
    docker compose -f "$COMPOSE_FILE" "$@"
  fi
}

# Run psql against the running postgres service over docker compose exec,
# optionally injecting --env-file so compose can resolve required secrets.
# Extra args (e.g. -c "...") are forwarded; stdin is passed through so callers
# can pipe a SQL dump in.
psql_exec() {
  compose_cmd exec -T postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" "$@"
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

require_regular_file() {
  local path="$1" label="$2"
  if [[ ! -f "$path" || -L "$path" ]]; then
    echo "$label is missing or is not a regular file: $path" >&2
    return 1
  fi
}

validate_sql_dump() {
  local dump="$1"
  require_regular_file "$dump" "PostgreSQL backup" || return 1
  if [[ ! -s "$dump" ]]; then
    echo "Backup file $dump is empty; refusing to restore." >&2
    return 1
  fi
  # Plain-text pg_dump output has a stable prologue and completion footer.  A
  # footer check catches both a dump cut at a statement boundary and one cut
  # in the middle of a statement before any live schema is changed.
  if ! LC_ALL=C head -c 4096 -- "$dump" | grep -a -q -- 'PostgreSQL database dump'; then
    echo "$dump does not look like a pg_dump SQL backup; refusing to restore." >&2
    return 1
  fi
  if ! LC_ALL=C grep -a -q -- '^-- PostgreSQL database dump complete' "$dump"; then
    echo "$dump is incomplete (missing pg_dump completion footer); refusing to restore." >&2
    return 1
  fi
  # A header/footer pair without at least one SQL statement is not a usable
  # dump.  SET statements are emitted even for an empty database, so count any
  # statement terminator before the footer rather than requiring table data.
  if ! awk '
    /^-- PostgreSQL database dump complete/ { exit found ? 0 : 1 }
    index($0, ";") { found = 1 }
    END { if (!found) exit 1 }
  ' "$dump"; then
    echo "$dump contains no complete SQL statement; refusing to restore." >&2
    return 1
  fi
}

validate_archive() {
  local archive="$1"
  require_regular_file "$archive" "agent state archive" || return 1
  if [[ ! -s "$archive" ]]; then
    echo "Agent state archive is empty; refusing to restore." >&2
    return 1
  fi
  # Listing alone is not sufficient: gzip can have a valid header while the
  # compressed payload is truncated. Python reads every regular member below,
  # while also rejecting traversal, links, and special files before extraction.
  if ! python3 - "$archive" <<'PY'
import posixpath
import sys
import tarfile

archive = sys.argv[1]
try:
    with tarfile.open(archive, mode="r:gz") as bundle:
        members = bundle.getmembers()
        if not members:
            raise ValueError("archive contains no members")
        for member in members:
            name = member.name
            if (
                not name
                or "\x00" in name
                or "\\" in name
                or name.startswith("/")
                or posixpath.normpath(name) == ".."
                or posixpath.normpath(name).startswith("../")
            ):
                raise ValueError(f"unsafe archive member: {name!r}")
            if member.issym() or member.islnk() or not (member.isdir() or member.isfile()):
                raise ValueError(f"unsupported archive member: {name!r}")
            if member.isfile():
                stream = bundle.extractfile(member)
                if stream is None:
                    raise ValueError(f"unable to read archive member: {name!r}")
                while stream.read(1024 * 1024):
                    pass
except (OSError, tarfile.TarError, ValueError) as error:
    print(f"invalid agent state archive: {error}", file=sys.stderr)
    raise SystemExit(1)
PY
  then
    echo "Invalid agent state archive; refusing to restore." >&2
    return 1
  fi
}

# Import the dump into a disposable database before touching the live schema.
# The subshell owns an EXIT trap so a failed import still drops the temporary
# database before the restore can proceed (or return the failure to the caller).
validate_sql_semantics() (
  local dump="$1"
  local validation_db="hostlet_restore_validation_${BASHPID}_${RANDOM}"
  local validation_created=false

  cleanup_validation_db() {
    local status=$?
    trap - EXIT INT TERM
    if [[ "$validation_created" != true ]]; then
      exit "$status"
    fi
    set +e
    local drop_status=1
    for attempt in 1 2; do
      compose_cmd exec -T postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
        -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$validation_db\";" >/dev/null
      drop_status=$?
      if ((drop_status == 0)); then
        break
      fi
      if ((attempt == 1)); then
        echo "Temporary restore validation database cleanup failed; retrying." >&2
      fi
    done
    if ((drop_status == 0)); then
      validation_created=false
    fi
    if ((drop_status != 0)); then
      echo "Unable to remove temporary restore validation database; refusing to continue." >&2
      if ((status == 0)); then
        status=$drop_status
      fi
    fi
    exit "$status"
  }
  trap cleanup_validation_db EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  if ! compose_cmd exec -T postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
    -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$validation_db\";" >/dev/null; then
    echo "Unable to create temporary restore validation database; refusing to continue." >&2
    exit 1
  fi
  validation_created=true

  if ! compose_cmd exec -T postgres psql -U "$POSTGRES_USER" -d "$validation_db" \
    -v ON_ERROR_STOP=1 --single-transaction < "$dump"; then
    echo "PostgreSQL backup failed semantic validation; refusing to restore." >&2
    exit 1
  fi
)

# Validate every supplied artifact before asking for confirmation or issuing a
# command that can mutate the live database or Docker volumes. SQL is then
# imported into a disposable database so header/footer checks cannot bless
# malformed statements.
validate_sql_dump "$BACKUP_DIR/postgres.sql"
if [[ -e "$BACKUP_DIR/hostlet-agent-state.tar.gz" || -L "$BACKUP_DIR/hostlet-agent-state.tar.gz" ]]; then
  validate_archive "$BACKUP_DIR/hostlet-agent-state.tar.gz"
fi
validate_sql_semantics "$BACKUP_DIR/postgres.sql"

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
