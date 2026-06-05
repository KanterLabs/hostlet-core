#!/usr/bin/env bash
set -euo pipefail

allowed_names="${HOSTLET_ALLOWED_RUNNER_NAMES:-}"
expected_os="${HOSTLET_EXPECTED_RUNNER_OS:-Linux}"
expected_arch="${HOSTLET_EXPECTED_RUNNER_ARCH:-X64}"

if [ -z "${RUNNER_NAME:-}" ]; then
  echo "RUNNER_NAME is not set; this check must run inside GitHub Actions" >&2
  exit 1
fi

if [ -z "${RUNNER_OS:-}" ] || [ -z "${RUNNER_ARCH:-}" ]; then
  echo "RUNNER_OS/RUNNER_ARCH are not set; this check must run inside GitHub Actions" >&2
  exit 1
fi

if [ "${RUNNER_OS}" != "${expected_os}" ]; then
  echo "unexpected runner OS: got ${RUNNER_OS}, expected ${expected_os}" >&2
  exit 1
fi

if [ "${RUNNER_ARCH}" != "${expected_arch}" ]; then
  echo "unexpected runner arch: got ${RUNNER_ARCH}, expected ${expected_arch}" >&2
  exit 1
fi

if [ -n "${allowed_names}" ]; then
  IFS=',' read -r -a names <<< "${allowed_names}"
  matched=0
  for name in "${names[@]}"; do
    if [ "${RUNNER_NAME}" = "${name}" ]; then
      matched=1
      break
    fi
  done
  if [ "${matched}" -ne 1 ]; then
    echo "unexpected runner name: got ${RUNNER_NAME}, allowed ${allowed_names}" >&2
    exit 1
  fi
fi

if [ "${HOSTLET_ALLOW_LOW_DISK:-0}" != "1" ]; then
  disk_use_percent="$(df -P / | awk 'NR == 2 { gsub("%", "", $5); print $5 }')"
  disk_fail_percent="${HOSTLET_RUNNER_DISK_FAIL_PERCENT:-92}"
  if [ -n "${disk_use_percent}" ] && [ "${disk_use_percent}" -ge "${disk_fail_percent}" ]; then
    echo "runner root disk is ${disk_use_percent}% full; refusing CI above ${disk_fail_percent}%" >&2
    exit 1
  fi
fi

echo "verified self-hosted runner ${RUNNER_NAME} (${RUNNER_OS}/${RUNNER_ARCH})"
