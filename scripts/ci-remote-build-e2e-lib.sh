#!/usr/bin/env bash

remote_e2e_runner_get() {
  local runner_container="$1"
  local url="$2"
  local max_attempts="${3:-10}"
  local retry_delay="${4:-1}"
  local attempt body

  case "${max_attempts}" in
    ''|*[!0-9]*|0)
      echo "remote runtime probe attempts must be a positive integer" >&2
      return 2
      ;;
  esac

  for ((attempt = 1; attempt <= max_attempts; attempt += 1)); do
    if body="$(docker exec "${runner_container}" wget -qO- "${url}" 2>/dev/null)"; then
      printf '%s' "${body}"
      return 0
    fi
    if ((attempt < max_attempts)); then
      sleep "${retry_delay}"
    fi
  done

  echo "remote runtime probe failed after ${max_attempts} attempts: ${url}" >&2
  return 1
}
