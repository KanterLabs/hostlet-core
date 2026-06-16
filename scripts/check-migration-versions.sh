#!/bin/sh
set -eu

# Migration-version dup guard (intra-repo, zero cloud knowledge).
#
# sqlx tracks each migration by its leading NNN number, so two migrations that
# share a number collide on the _sqlx_migrations primary key and crash the API
# on boot. This guard fails fast if any two `apps/api/migrations/NNN_*.sql`
# files in THIS repo share a number. The cloud overlay has its own (separate)
# cross-repo gate over the merged core+cloud set; this one stays generic so a
# self-hosted core checkout is protected on its own.
#
# Portable on purpose: POSIX sh + sed/sort/uniq only (no bash, no arrays).

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MIGRATIONS_DIR="${ROOT}/apps/api/migrations"

if [ ! -d "${MIGRATIONS_DIR}" ]; then
  echo "check-migration-versions: missing ${MIGRATIONS_DIR}" >&2
  exit 1
fi

# Anchor the prefix as exactly three digits + underscore so a malformed name
# (e.g. `27_foo.sql` or `0001_foo.sql`) can't masquerade as a valid number and
# slip past the dup check.
numbers="$(
  for f in "${MIGRATIONS_DIR}"/*.sql; do
    [ -e "${f}" ] || continue
    base="$(basename "${f}")"
    case "${base}" in
      [0-9][0-9][0-9]_*) printf '%s\n' "${base}" | sed -n 's/^\([0-9][0-9][0-9]\)_.*/\1/p' ;;
      *)
        echo "check-migration-versions: migration name not NNN_*.sql: ${base}" >&2
        exit 1
        ;;
    esac
  done
)"

dups="$(printf '%s\n' "${numbers}" | sort | uniq -d)"

if [ -n "${dups}" ]; then
  echo "check-migration-versions: duplicate migration numbers in apps/api/migrations:" >&2
  for n in ${dups}; do
    echo "  number ${n}:" >&2
    for f in "${MIGRATIONS_DIR}/${n}"_*.sql; do
      [ -e "${f}" ] || continue
      echo "    $(basename "${f}")" >&2
    done
  done
  echo "Two migrations sharing a number collide on _sqlx_migrations_pkey and crash the API on boot." >&2
  exit 1
fi

echo "check-migration-versions: no duplicate migration numbers"
