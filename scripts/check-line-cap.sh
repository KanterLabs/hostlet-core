#!/usr/bin/env bash
set -euo pipefail

limit="${HOSTLET_LINE_CAP:-1000}"
fail=0

is_authored_file() {
  case "$1" in
    Cargo.lock|*"/Cargo.lock"|*"/pnpm-lock.yaml"|*"/package-lock.json"|*"/yarn.lock") return 1 ;;
    *".tsbuildinfo"|*".min.js"|*".map") return 1 ;;
    *.rs|*.ts|*.tsx|*.js|*.mjs|*.sh|*.sql|*.yml|*.yaml|*.toml|*.json|*.md|*.css) return 0 ;;
    *) return 1 ;;
  esac
}

while IFS= read -r file; do
  [ -f "$file" ] || continue
  is_authored_file "$file" || continue
  lines="$(wc -l < "$file" | tr -d ' ')"
  if [ "$lines" -gt "$limit" ]; then
    printf '%s\t%s\n' "$lines" "$file"
    fail=1
  fi
done < <(git ls-files)

if [ "$fail" -ne 0 ]; then
  echo "Authored files above ${limit} lines must be decomposed." >&2
  exit 1
fi
