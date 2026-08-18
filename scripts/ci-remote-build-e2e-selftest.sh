#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/ci-remote-build-e2e-lib.sh
source "${ROOT}/scripts/ci-remote-build-e2e-lib.sh"

TMP_DIR="$(mktemp -d)"
CALLS="${TMP_DIR}/calls"
trap 'rm -rf "${TMP_DIR}"' EXIT

sleep() { :; }

docker() {
  printf 'attempt\n' >> "${CALLS}"
  if [ "$(wc -l < "${CALLS}")" -lt 3 ]; then
    return 1
  fi
  printf 'healthy\n'
}

output="$(remote_e2e_runner_get runner http://runtime.test/health 5 0)"
test "${output}" = "healthy"
test "$(wc -l < "${CALLS}")" -eq 3

: > "${CALLS}"
docker() {
  printf 'attempt\n' >> "${CALLS}"
  return 1
}

if remote_e2e_runner_get runner http://runtime.test/health 4 0 \
  >"${TMP_DIR}/unexpected-output" 2>"${TMP_DIR}/failure"; then
  echo "permanent runtime probe failure unexpectedly succeeded" >&2
  exit 1
fi
test "$(wc -l < "${CALLS}")" -eq 4
grep -q 'failed after 4 attempts' "${TMP_DIR}/failure"

if remote_e2e_runner_get runner http://runtime.test/health invalid 0 \
  >"${TMP_DIR}/unexpected-output" 2>"${TMP_DIR}/invalid"; then
  echo "invalid retry limit unexpectedly succeeded" >&2
  exit 1
fi
grep -q 'positive integer' "${TMP_DIR}/invalid"

test "$(grep -c 'source .*ci-remote-build-e2e-lib.sh' "${ROOT}/scripts/ci-remote-build-e2e.sh")" -eq 1
if grep -q 'docker exec .* wget -qO-' "${ROOT}/scripts/ci-remote-build-e2e.sh"; then
  echo "remote E2E contains a non-retrying runtime probe" >&2
  exit 1
fi
test "$(grep -c 'remote_e2e_runner_get' "${ROOT}/scripts/ci-remote-build-e2e.sh")" -eq 5

echo "remote build E2E retry self-test passed"
