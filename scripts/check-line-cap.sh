#!/usr/bin/env bash
set -euo pipefail

# Line-cap and fake-module guard.
#
# WHY THE LINE CAPS: a hard ceiling forces real domain decomposition instead of
# letting any single authored file grow without bound. ${limit} (default 1000)
# is the CI-failing ceiling; ${warn_limit} (default 750) is a soft heads-up at
# 75% of the cap so authors can split a file *before* it trips the gate. Both are
# overridable via env vars only so the policy lives in CI config, not per-repo.
limit="${HOSTLET_LINE_CAP:-1000}"
warn_limit="${HOSTLET_WARN_LINE_CAP:-750}"
fail=0
fake_modules=0
warned=0

# Files we hold to the line cap: human-authored source/config/doc formats.
# Everything else (vendored trees, generated lockfiles, build/minified output,
# source maps) is machine-managed, so its length is not a maintainability signal
# and is excluded.
is_authored_file() {
  case "$1" in
    vendor/*) return 1 ;;
    Cargo.lock|*"/Cargo.lock"|*"/pnpm-lock.yaml"|*"/package-lock.json"|*"/yarn.lock") return 1 ;;
    *".tsbuildinfo"|*".min.js"|*".map") return 1 ;;
    *.rs|*.ts|*.tsx|*.js|*.mjs|*.sh|*.sql|*.yml|*.yaml|*.toml|*.json|*.md|*.css) return 0 ;;
    *) return 1 ;;
  esac
}

# WHY THE FAKE-MODULE BANS: the cap must not be dodged by mechanically slicing one
# logical file into shards that are stitched back together at compile time. Two
# such evasions are forbidden:
#   1. `*_partN.rs` filenames  - a monofile split into foo_part1.rs, foo_part2.rs.
#   2. `include!("..._partN.rs")` - the textual re-glue that reassembles the shards.
# Either pattern is a disguised split rather than a cohesive domain module, so the
# guard rejects it outright (separately from the length check below).
while IFS= read -r file; do
  [ -f "$file" ] || continue
  case "$file" in
    *_part[0-9]*.rs)
      printf 'fake part module\t%s\n' "$file"
      fake_modules=1
      ;;
  esac
  case "$file" in
    *.rs)
      if grep -Eq 'include!\("[^"]*_part[0-9][^"]*\.rs"\)' "$file"; then
        printf 'fake include module\t%s\n' "$file"
        fake_modules=1
      fi
      ;;
  esac
  is_authored_file "$file" || continue
  lines="$(wc -l < "$file" | tr -d ' ')"
  if [ "$lines" -gt "$limit" ]; then
    printf '%s\t%s\n' "$lines" "$file"
    fail=1
  elif [ "$lines" -gt "$warn_limit" ]; then
    printf 'warning: %s lines\t%s\n' "$lines" "$file" >&2
    warned=1
  fi
done < <(git ls-files --cached --others --exclude-standard)

if [ "$fake_modules" -ne 0 ]; then
  echo "Rust *_partN.rs include shards are not real modularization. Use domain modules instead." >&2
  exit 1
fi

if [ "$fail" -ne 0 ]; then
  echo "Authored files above ${limit} lines must be decomposed." >&2
  exit 1
fi

if [ "$warned" -ne 0 ]; then
  echo "Authored files above ${warn_limit} lines are close to the decomposition boundary." >&2
fi
