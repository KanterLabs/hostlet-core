#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_WORKFLOW="${ROOT}/.github/workflows/staging.yml"
SELF_HOSTED_LIB="${ROOT}/scripts/ci-self-hosted-lib.sh"
CI_WORKFLOW="${ROOT}/.github/workflows/ci.yml"
STAGING_DEPLOYABILITY="${ROOT}/.github/workflows/deployability.yml"
FULL_CI_WORKFLOW="${ROOT}/.github/workflows/full-ci.yml"

assert_contains() {
  local file="$1"
  local needle="$2"
  if ! grep -Fq -- "${needle}" "${file}"; then
    echo "${file#${ROOT}/} missing expected text: ${needle}" >&2
    exit 1
  fi
}

assert_contains "${SELF_HOSTED_LIB}" 'ensure_rust_toolchain_path'
assert_contains "${SELF_HOSTED_LIB}" 'export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"'
assert_contains "${SELF_HOSTED_LIB}" 'export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"'
assert_contains "${SELF_HOSTED_LIB}" 'ci_cargo()'
assert_contains "${SELF_HOSTED_LIB}" 'ci_tmp_dir()'
assert_contains "${SELF_HOSTED_LIB}" 'local parent="${RUNNER_TEMP:-/tmp}"'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-verify-runner-selftest.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-screenshotter-smoke.sh'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-core-workflow-contracts.sh'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-verify-runner-selftest.sh'
assert_contains "${STAGING_DEPLOYABILITY}" 'runs-on: [self-hosted, Linux, X64, hostlet-core]'
assert_contains "${FULL_CI_WORKFLOW}" 'runs-on: [self-hosted, Linux, X64, hostlet-core]'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'TMP_DIR="$(ci_tmp_dir hostlet-self-api "${RUN_ID}")"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'TMP_DIR="$(ci_tmp_dir hostlet-self-deploy "${RUN_ID}")"'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'timed out waiting for self-hosted API'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'HOSTLET_SELF_HOSTED_AGENT_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'timed out waiting for self-hosted agent'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'ci_cargo run -p hostlet-api'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ci_cargo run -p hostlet-api'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ci_cargo run -p hostlet-agent'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'docker info'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'mountpoint -q /var/lib/docker'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'HOSTLET_RUNNER_DOCKER_DISK_FAIL_PERCENT'
assert_contains "${ROOT}/scripts/ci-verify-runner-selftest.sh" 'STUB_DOCKER_FAIL=1'
assert_contains "${ROOT}/scripts/ci-verify-runner-selftest.sh" 'HOSTLET_ALLOW_LOW_DOCKER_DISK=1'

python3 - "${STAGING_WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
match = re.search(r'-d\s+"(?P<payload>\{.*core-staging-updated.*\})"', workflow)
if not match:
    raise SystemExit("staging workflow missing repository_dispatch JSON payload")

payload = match.group("payload").replace(r'\"', '"')
required = [
    '"event_type":"core-staging-updated"',
    '"schema_version":1',
    '"core_sha":"${GITHUB_SHA}"',
    '"core_tag":"sha-${GITHUB_SHA:0:12}"',
]
for needle in required:
    if needle not in payload:
        raise SystemExit(f"dispatch payload missing {needle}")
PY

echo "core workflow contracts passed"
