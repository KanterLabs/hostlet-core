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

echo "backup destination protection self-test passed"
