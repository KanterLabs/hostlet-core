#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 64
fi

delays=(0 15 30 60 120)
last_status=1

for index in "${!delays[@]}"; do
  delay="${delays[$index]}"
  attempt="$((index + 1))"
  if [ "${delay}" -gt 0 ]; then
    echo "Retrying docker command in ${delay}s (attempt ${attempt}/${#delays[@]})..." >&2
    sleep "${delay}"
  fi
  if "$@"; then
    exit 0
  else
    last_status="$?"
  fi
  echo "Docker command failed with exit ${last_status} (attempt ${attempt}/${#delays[@]})." >&2
done

exit "${last_status}"
