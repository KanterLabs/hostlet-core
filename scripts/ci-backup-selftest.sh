#!/usr/bin/env bash
set -euo pipefail

# Focused backup regression harness. It replaces Docker with a tiny command
# shim so destination ownership and failure cleanup can be exercised in CI.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-backup-selftest.XXXXXX")"
trap 'rm -rf -- "$TMP_DIR"' EXIT
FAKE_BIN="$TMP_DIR/bin"
mkdir -m 700 "$FAKE_BIN"

cat > "$FAKE_BIN/docker" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == compose ]]; then
  if [[ "${HOSTLET_BACKUP_TEST_FAIL:-}" == compose ]]; then
    exit 42
  fi
  printf '%s\n' '-- PostgreSQL database dump' '-- Hostlet backup self-test'
  exit 0
fi
if [[ "${1:-}" == volume && "${2:-}" == inspect ]]; then
  exit 1
fi
exit 0
SHIM
chmod 700 "$FAKE_BIN/docker"

BACKUP_ROOT="$TMP_DIR/backups"
mkdir -m 700 -p "$BACKUP_ROOT"
existing="$BACKUP_ROOT/existing"
mkdir -m 700 "$existing"
printf '%s\n' sentinel > "$existing/sentinel.txt"

if PATH="$FAKE_BIN:$PATH" HOSTLET_BACKUP_ROOT="$BACKUP_ROOT" \
  bash "$ROOT_DIR/scripts/backup.sh" "$existing" >/dev/null 2>&1; then
  echo "backup unexpectedly accepted an existing destination" >&2
  exit 1
fi
[[ "$(cat "$existing/sentinel.txt")" == sentinel ]]

failed="$BACKUP_ROOT/failed"
if PATH="$FAKE_BIN:$PATH" HOSTLET_BACKUP_ROOT="$BACKUP_ROOT" \
  HOSTLET_BACKUP_TEST_FAIL=compose bash "$ROOT_DIR/scripts/backup.sh" "$failed" >/dev/null 2>&1; then
  echo "backup unexpectedly succeeded after pg_dump failure" >&2
  exit 1
fi
[[ ! -e "$failed" ]]
if compgen -G "$BACKUP_ROOT/.hostlet-backup-tmp.*" >/dev/null; then
  echo "failed backup left an owned staging directory" >&2
  exit 1
fi

pointer="$BACKUP_ROOT/latest.json"
printf '%s\n' caller-owned-pointer > "$pointer"
published="$BACKUP_ROOT/published"
if PATH="$FAKE_BIN:$PATH" HOSTLET_BACKUP_ROOT="$BACKUP_ROOT" \
  HOSTLET_BACKUP_TEST_FAIL_AT=metadata-write bash "$ROOT_DIR/scripts/backup.sh" "$published" >/dev/null 2>&1; then
  echo "backup unexpectedly succeeded after metadata failure injection" >&2
  exit 1
fi
[[ "$(cat "$pointer")" == caller-owned-pointer ]]
[[ -d "$published" ]]
if compgen -G "$BACKUP_ROOT/.hostlet-latest.*" >/dev/null; then
  echo "metadata failure left an owned pointer staging file" >&2
  exit 1
fi

cat > "$FAKE_BIN/docker" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == compose ]]; then
  printf '%s\n' '-- PostgreSQL database dump' '-- Hostlet backup self-test'
  exit 0
fi
if [[ "${1:-}" == volume && "${2:-}" == inspect ]]; then
  [[ "${3:-}" == infra_hostlet-agent || "${3:-}" == infra_hostlet-screenshots ]]
  exit
fi
if [[ "${1:-}" == run ]]; then
  backup_mount=""
  args=("$@")
  for ((index = 1; index < ${#args[@]}; index++)); do
    if [[ "${args[index]}" == -v && "${args[index + 1]}" == *:/backup ]]; then
      backup_mount="${args[index + 1]%:/backup}"
      break
    fi
  done
  [[ -n "$backup_mount" ]] || exit 1
  if [[ "$*" == *hostlet-agent-state.tar.gz* ]]; then
    printf '%s\n' agent-state > "$backup_mount/hostlet-agent-state.tar.gz"
  fi
  if [[ "$*" == *hostlet-screenshots-state.tar.gz* ]]; then
    tar -czf "$backup_mount/hostlet-screenshots-state.tar.gz" -C "${HOSTLET_SCREENSHOT_SOURCE:?}" .
  fi
  exit 0
fi
exit 0
SHIM
chmod 700 "$FAKE_BIN/docker"
screenshot_source="$TMP_DIR/screenshot-source"
mkdir -m 700 "$screenshot_source"
printf 'hostlet-screenshot-bytes\0\377\n' > "$screenshot_source/screenshot.webp"
screenshot_backup_root="$TMP_DIR/screenshot-backups"
screenshot_backup="$screenshot_backup_root/snapshot"
HOSTLET_SCREENSHOT_SOURCE="$screenshot_source" PATH="$FAKE_BIN:$PATH" HOSTLET_BACKUP_ROOT="$screenshot_backup_root" \
  bash "$ROOT_DIR/scripts/backup.sh" "$screenshot_backup" >/dev/null
[[ -s "$screenshot_backup/hostlet-agent-state.tar.gz" ]]
[[ -s "$screenshot_backup/hostlet-screenshots-state.tar.gz" ]]
grep -q '^screenshot_volume=infra_hostlet-screenshots$' "$screenshot_backup/manifest.txt"
screenshot_archive_extract="$TMP_DIR/screenshot-archive-extract"
mkdir -m 700 "$screenshot_archive_extract"
tar -xzf "$screenshot_backup/hostlet-screenshots-state.tar.gz" -C "$screenshot_archive_extract"
cmp "$screenshot_source/screenshot.webp" "$screenshot_archive_extract/screenshot.webp"

restore_log="$TMP_DIR/restore.log"
cat > "$FAKE_BIN/docker" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${HOSTLET_RESTORE_TEST_LOG:?}"
db_state="${HOSTLET_RESTORE_TEST_DATABASES:-}"
create_pattern='CREATE DATABASE "([^"]+)"'
drop_pattern='DROP DATABASE IF EXISTS "([^"]+)"'
if [[ -n "$db_state" && "$*" =~ $create_pattern ]]; then
  printf '%s\n' "${BASH_REMATCH[1]}" >> "$db_state"
  exit 0
fi
if [[ -n "$db_state" && "$*" =~ $drop_pattern ]]; then
  if [[ "${HOSTLET_RESTORE_TEST_FAIL_DROP_ONCE:-}" == yes && ! -e "${HOSTLET_RESTORE_TEST_DROP_MARKER:?}" ]]; then
    : > "$HOSTLET_RESTORE_TEST_DROP_MARKER"
    exit 46
  fi
  db_name="${BASH_REMATCH[1]}"
  db_state_tmp="${db_state}.tmp"
  awk -v target="$db_name" '$0 != target' "$db_state" > "$db_state_tmp"
  mv -T "$db_state_tmp" "$db_state"
  exit 0
fi
if [[ "$*" == *restore_validation_* && "$*" == *--single-transaction* ]]; then
  semantic_sql="$(cat)"
  if grep -Eq '(^|[[:space:]])SELEKT([[:space:]]|$)' <<< "$semantic_sql"; then
    exit 42
  fi
fi
if [[ "$*" == *"ps --status running --services"* ]]; then
  if [[ "${HOSTLET_RESTORE_TEST_FAIL:-}" == probe ]]; then
    exit 45
  fi
  service="${*: -1}"
  case ",${HOSTLET_RESTORE_TEST_RUNNING:-}," in
    *,"$service",*) printf '%s\n' "$service" ;;
  esac
  exit 0
fi
if [[ "${HOSTLET_RESTORE_TEST_FAIL:-}" == import && "$*" == *--single-transaction* && "$*" != *restore_validation_* ]]; then
  exit 42
fi
if [[ "${HOSTLET_RESTORE_TEST_FAIL:-}" == archive && "$*" == *"tar -xzf /backup/hostlet-agent-state.tar.gz"* ]]; then
  exit 43
fi
if [[ "${1:-}" == run && "$*" == *hostlet-screenshots-state.tar.gz* ]]; then
  backup_mount=""
  args=("$@")
  for ((index = 1; index < ${#args[@]}; index++)); do
    if [[ "${args[index]}" == -v && "${args[index + 1]}" == *:/backup:ro ]]; then
      backup_mount="${args[index + 1]%:/backup:ro}"
      break
    fi
  done
  [[ -n "$backup_mount" && -n "${HOSTLET_SCREENSHOT_RESTORE_DIR:-}" ]] || exit 1
  rm -rf -- "$HOSTLET_SCREENSHOT_RESTORE_DIR"
  mkdir -m 700 -p "$HOSTLET_SCREENSHOT_RESTORE_DIR"
  tar -xzf "$backup_mount/hostlet-screenshots-state.tar.gz" -C "$HOSTLET_SCREENSHOT_RESTORE_DIR"
fi
exit 0
SHIM
chmod 700 "$FAKE_BIN/docker"

assert_restore_rejected() {
  local name="$1"
  : > "$restore_log"
  if PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$restore_log" \
    bash "$ROOT_DIR/scripts/restore.sh" "$TMP_DIR/restore-$name" >/dev/null 2>&1; then
    echo "restore unexpectedly accepted $name" >&2
    exit 1
  fi
  [[ ! -s "$restore_log" ]] || {
    echo "restore touched Docker before rejecting $name" >&2
    exit 1
  }
}

for name in empty header-only statement-boundary-truncated mid-statement-truncated; do
  mkdir -m 700 "$TMP_DIR/restore-$name"
done
touch "$TMP_DIR/restore-empty/postgres.sql"
printf '%s\n' '-- PostgreSQL database dump' > "$TMP_DIR/restore-header-only/postgres.sql"
printf '%s\n' '-- PostgreSQL database dump' 'CREATE TABLE sample (id integer);' \
  > "$TMP_DIR/restore-statement-boundary-truncated/postgres.sql"
printf '%s\n' '-- PostgreSQL database dump' 'CREATE TABLE sample (id integer' \
  > "$TMP_DIR/restore-mid-statement-truncated/postgres.sql"
for name in empty header-only statement-boundary-truncated mid-statement-truncated; do
  assert_restore_rejected "$name"
done

mkdir -m 700 "$TMP_DIR/restore-invalid-sql"
printf '%s\n' '-- PostgreSQL database dump' 'SELEKT invalid SQL;' \
  '-- PostgreSQL database dump complete' > "$TMP_DIR/restore-invalid-sql/postgres.sql"
semantic_state="$TMP_DIR/semantic-databases"
: > "$semantic_state"
: > "$restore_log"
if PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$restore_log" \
  HOSTLET_RESTORE_TEST_DATABASES="$semantic_state" bash "$ROOT_DIR/scripts/restore.sh" \
  "$TMP_DIR/restore-invalid-sql" >/dev/null 2>&1; then
  echo "restore unexpectedly accepted semantically invalid SQL" >&2
  exit 1
fi
grep -q 'CREATE DATABASE "hostlet_restore_validation_' "$restore_log"
grep -q 'DROP DATABASE IF EXISTS "hostlet_restore_validation_' "$restore_log"
! grep -q 'DROP SCHEMA public CASCADE' "$restore_log"
! grep -q '^volume create ' "$restore_log"
[[ ! -s "$semantic_state" ]]

mkdir -m 700 "$TMP_DIR/restore-corrupt-archive"
printf '%s\n' '-- PostgreSQL database dump' 'SET client_encoding = '\''UTF8'\'';' \
  '-- PostgreSQL database dump complete' > "$TMP_DIR/restore-corrupt-archive/postgres.sql"
printf '%s\n' 'not a gzip archive' > "$TMP_DIR/restore-corrupt-archive/hostlet-agent-state.tar.gz"
assert_restore_rejected corrupt-archive

mkdir -m 700 "$TMP_DIR/restore-truncated-gzip"
printf '%s\n' '-- PostgreSQL database dump' 'SET client_encoding = '\''UTF8'\'';' \
  '-- PostgreSQL database dump complete' > "$TMP_DIR/restore-truncated-gzip/postgres.sql"
archive_source="$TMP_DIR/archive-source"
mkdir -m 700 "$archive_source"
printf '%s\n' archive-member > "$archive_source/state.txt"
valid_archive="$TMP_DIR/valid-state.tar.gz"
tar -czf "$valid_archive" -C "$archive_source" .
archive_bytes="$(wc -c < "$valid_archive")"
dd if="$valid_archive" of="$TMP_DIR/restore-truncated-gzip/hostlet-agent-state.tar.gz" \
  bs=1 count=$((archive_bytes / 2)) status=none
assert_restore_rejected truncated-gzip

cleanup_restore="$TMP_DIR/restore-cleanup-retry"
mkdir -m 700 "$cleanup_restore"
printf '%s\n' '-- PostgreSQL database dump' 'SET client_encoding = '\''UTF8'\'';' \
  '-- PostgreSQL database dump complete' > "$cleanup_restore/postgres.sql"
cleanup_state="$TMP_DIR/cleanup-databases"
cleanup_marker="$TMP_DIR/cleanup-drop-once"
cleanup_log="$TMP_DIR/cleanup.log"
cleanup_output="$TMP_DIR/cleanup-output"
: > "$cleanup_state"
: > "$cleanup_log"
PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$cleanup_log" \
  HOSTLET_RESTORE_TEST_DATABASES="$cleanup_state" HOSTLET_RESTORE_TEST_FAIL_DROP_ONCE=yes \
  HOSTLET_RESTORE_TEST_DROP_MARKER="$cleanup_marker" \
  bash "$ROOT_DIR/scripts/restore.sh" "$cleanup_restore" > "$cleanup_output"
grep -q 'Restore complete\.' "$cleanup_output"
[[ -e "$cleanup_marker" ]]
[[ ! -s "$cleanup_state" ]]
[[ "$(grep -c 'DROP DATABASE IF EXISTS "hostlet_restore_validation_' "$cleanup_log")" -eq 2 ]]

restore_ok="$TMP_DIR/restore-valid"
mkdir -m 700 "$restore_ok"
printf '%s\n' '-- PostgreSQL database dump' 'SET client_encoding = '\''UTF8'\'';' \
  '-- PostgreSQL database dump complete' > "$restore_ok/postgres.sql"
agent_archive_source="$TMP_DIR/agent-state"
mkdir -m 700 "$agent_archive_source"
printf '%s\n' restored-agent-state > "$agent_archive_source/state.json"
tar -czf "$restore_ok/hostlet-agent-state.tar.gz" -C "$agent_archive_source" .
cp "$screenshot_backup/hostlet-screenshots-state.tar.gz" \
  "$restore_ok/hostlet-screenshots-state.tar.gz"
restore_journal="$TMP_DIR/restore-journal"
: > "$restore_log"
probe_output="$TMP_DIR/probe-output"
if PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$restore_log" \
  HOSTLET_RESTORE_TEST_FAIL=probe HOSTLET_RESTORE_TEST_RUNNING=api,local-agent,caddy \
  HOSTLET_RESTORE_JOURNAL="$restore_journal" \
  bash "$ROOT_DIR/scripts/restore.sh" "$restore_ok" > "$probe_output" 2>&1; then
  echo "restore unexpectedly proceeded after writer-state probe failure" >&2
  exit 1
fi
grep -q "Unable to determine whether writer service 'api' is running" "$probe_output"
! grep -q ' stop -t ' "$restore_log"
! grep -q 'DROP SCHEMA public CASCADE' "$restore_log"
! grep -q '^volume create ' "$restore_log"
[[ ! -e "$restore_journal" ]]

: > "$restore_log"
if PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$restore_log" \
  HOSTLET_RESTORE_TEST_FAIL=import HOSTLET_RESTORE_TEST_RUNNING=api,local-agent,caddy \
  HOSTLET_RESTORE_JOURNAL="$restore_journal" \
  bash "$ROOT_DIR/scripts/restore.sh" "$restore_ok" >/dev/null 2>&1; then
  echo "restore unexpectedly succeeded after import failure" >&2
  exit 1
fi
grep -q 'stop .*api web local-agent caddy' "$restore_log"
grep -q 'start api local-agent caddy' "$restore_log"
! grep -q 'start api web local-agent caddy' "$restore_log"
grep -q '^phase=failed$' "$restore_journal"
grep -q '^services=services-restarted$' "$restore_journal"
rm -f "$restore_journal"

: > "$restore_log"
archive_output="$TMP_DIR/archive-restore-output"
if PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$restore_log" \
  HOSTLET_RESTORE_TEST_FAIL=archive HOSTLET_RESTORE_TEST_RUNNING=api,local-agent,caddy \
  HOSTLET_RESTORE_JOURNAL="$restore_journal" \
  bash "$ROOT_DIR/scripts/restore.sh" "$restore_ok" > "$archive_output" 2>&1; then
  echo "restore unexpectedly succeeded after archive extraction failure" >&2
  exit 1
fi
grep -q 'stop .*api web local-agent caddy' "$restore_log"
grep -q 'start api local-agent caddy' "$restore_log"
! grep -q 'start api web local-agent caddy' "$restore_log"
grep -q '^phase=failed$' "$restore_journal"
grep -q '^services=services-restarted$' "$restore_journal"
grep -q '^recovery=database-or-agent-state-may-be-partial$' "$restore_journal"
grep -q '^original_running=api,local-agent,caddy$' "$restore_journal"
grep -q 'originally running services were restarted' "$archive_output"
rm -f "$restore_journal"

: > "$restore_log"
restore_output="$TMP_DIR/restore-output"
screenshot_restore_dir="$TMP_DIR/screenshot-restored"
mkdir -m 700 "$screenshot_restore_dir"
[[ -z "$(find "$screenshot_restore_dir" -mindepth 1 -print -quit)" ]]
PATH="$FAKE_BIN:$PATH" HOSTLET_RESTORE_CONFIRM=yes HOSTLET_RESTORE_TEST_LOG="$restore_log" \
  HOSTLET_SCREENSHOT_RESTORE_DIR="$screenshot_restore_dir" \
  HOSTLET_RESTORE_TEST_RUNNING=api,local-agent,caddy \
  HOSTLET_RESTORE_JOURNAL="$restore_journal" \
  bash "$ROOT_DIR/scripts/restore.sh" "$restore_ok" > "$restore_output"
grep -q 'Restore complete\.' "$restore_output"
grep -q 'stop .*api web local-agent caddy' "$restore_log"
grep -q 'start api local-agent caddy' "$restore_log"
grep -q 'infra_hostlet-screenshots:/data' "$restore_log"
cmp "$screenshot_source/screenshot.webp" "$screenshot_restore_dir/screenshot.webp"
[[ ! -e "$restore_journal" ]]

echo "backup destination protection self-test passed"
