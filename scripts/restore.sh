#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-$ROOT_DIR/infra/docker-compose.yml}"
POSTGRES_USER="${POSTGRES_USER:-hostlet}"
POSTGRES_DB="${POSTGRES_DB:-hostlet}"
AGENT_VOLUME="${HOSTLET_AGENT_VOLUME:-infra_hostlet-agent}"
AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-alpine:3.22}"
STOP_TIMEOUT="${HOSTLET_RESTORE_STOP_TIMEOUT_SECONDS:-30}"
JOURNAL_FILE="${HOSTLET_RESTORE_JOURNAL:-$ROOT_DIR/.hostlet/restore-state}"
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

if [[ ! "$STOP_TIMEOUT" =~ ^[1-9][0-9]{0,3}$ ]]; then
  echo "restore stop timeout must be between 1 and 9999 seconds" >&2
  exit 1
fi

JOURNAL_DIR="$(dirname -- "$JOURNAL_FILE")"
mkdir -p -- "$JOURNAL_DIR"
if [[ -e "$JOURNAL_FILE" || -L "$JOURNAL_FILE" ]]; then
  echo "an interrupted restore journal already exists at $JOURNAL_FILE; inspect it before retrying" >&2
  exit 1
fi

write_journal() {
  local phase="$1" services="$2" recovery="$3" temp
  temp="$(mktemp --tmpdir="$JOURNAL_DIR" .restore-state.XXXXXX)"
  chmod 600 -- "$temp"
  {
    printf 'format=hostlet-restore-v1\n'
    printf 'phase=%s\n' "$phase"
    printf 'services=%s\n' "$services"
    printf 'recovery=%s\n' "$recovery"
    printf 'original_running=%s\n' "${RUNNING_SERVICE_LIST:-none}"
    printf 'backup=%s\n' "$BACKUP_DIR"
    printf 'updated_at=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$temp"
  mv -T -- "$temp" "$JOURNAL_FILE"
}

STOPPED_SERVICES=(api web local-agent caddy)
RUNNING_SERVICES=()
RUNNING_SERVICE_LIST=none
SERVICES_STOPPED=false
RESTORE_OK=false
RESTORE_PHASE=validated

restore_exit() {
  local status=$?
  trap - EXIT
  trap '' INT TERM
  set +e
  if [[ "$RESTORE_OK" == true ]]; then
    rm -f -- "$JOURNAL_FILE"
  elif [[ "$SERVICES_STOPPED" == true ]]; then
    if ((${#RUNNING_SERVICES[@]} == 0)); then
      write_journal failed services-were-stopped "database-or-agent-state-may-be-partial"
      echo "Restore failed during ${RESTORE_PHASE}; no writer services were running before restore." >&2
      echo "Recoverable restore state is recorded at $JOURNAL_FILE; inspect it before retrying." >&2
    elif compose_cmd start "${RUNNING_SERVICES[@]}" >/dev/null 2>&1; then
      write_journal failed services-restarted "database-or-agent-state-may-be-partial"
      echo "Restore failed during ${RESTORE_PHASE}; the originally running services were restarted." >&2
      echo "Recoverable restore state is recorded at $JOURNAL_FILE; inspect it before retrying." >&2
    else
      write_journal recovery-required services-stopped "database-or-agent-state-may-be-partial"
      echo "Restore failed during ${RESTORE_PHASE}; API, web, agent, and router remain stopped." >&2
      echo "Recoverable restore state is recorded at $JOURNAL_FILE; restart the stack only after inspection." >&2
      status=1
    fi
  elif [[ "$SERVICES_STOPPED" == false ]]; then
    # Confirmation, service probing, or journal setup failed before any
    # service was stopped; do not leave a false services-stopped journal.
    rm -f -- "$JOURNAL_FILE"
  else
    write_journal failed services-running "no-writers-were-stopped"
  fi
  exit "$status"
}
trap restore_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ "${HOSTLET_RESTORE_CONFIRM:-}" != "yes" ]]; then
  echo "Refusing to restore without HOSTLET_RESTORE_CONFIRM=yes." >&2
  echo "This replaces the current Hostlet database contents." >&2
  exit 1
fi

RESTORE_PHASE=probing
for service in "${STOPPED_SERVICES[@]}"; do
  if ! running_services="$(compose_cmd ps --status running --services "$service")"; then
    echo "Unable to determine whether writer service '$service' is running; refusing to restore." >&2
    echo "No services were stopped and no live schema or volume was changed." >&2
    exit 1
  fi
  if [[ -n "$running_services" && "$running_services" != "$service" ]]; then
    echo "Unexpected writer service probe output for '$service'; refusing to restore." >&2
    echo "No services were stopped and no live schema or volume was changed." >&2
    exit 1
  fi
  if [[ "$running_services" == "$service" ]]; then
    RUNNING_SERVICES+=("$service")
  fi
done
if ((${#RUNNING_SERVICES[@]})); then
  RUNNING_SERVICE_LIST="$(IFS=,; printf '%s' "${RUNNING_SERVICES[*]}")"
fi
RESTORE_PHASE=quiescing
write_journal quiescing services-running "none"
SERVICES_STOPPED=true
compose_cmd stop -t "$STOP_TIMEOUT" "${STOPPED_SERVICES[@]}"
write_journal quiesced services-stopped "none"

RESTORE_PHASE=database-reset
psql_exec -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"

RESTORE_PHASE=database-import
psql_exec -v ON_ERROR_STOP=1 --single-transaction < "$BACKUP_DIR/postgres.sql"

RESTORE_PHASE=agent-state

if [[ -f "$BACKUP_DIR/hostlet-agent-state.tar.gz" ]]; then
  docker volume create "$AGENT_VOLUME" >/dev/null
  docker run --rm \
    -v "$AGENT_VOLUME:/data" \
    -v "$BACKUP_DIR:/backup:ro" \
    "$AGENT_IMAGE" \
    sh -lc 'rm -rf /data/* && tar -xzf /backup/hostlet-agent-state.tar.gz -C /data'
fi

RESTORE_PHASE=restart
if ((${#RUNNING_SERVICES[@]})); then
  compose_cmd start "${RUNNING_SERVICES[@]}"
fi
write_journal complete services-running "none"
RESTORE_OK=true
echo "Restore complete."
