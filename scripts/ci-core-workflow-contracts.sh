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
assert_contains "${SELF_HOSTED_LIB}" 'ci_binary_path()'
assert_contains "${SELF_HOSTED_LIB}" 'ci_build_binary()'
assert_contains "${SELF_HOSTED_LIB}" 'ci_tmp_dir()'
assert_contains "${SELF_HOSTED_LIB}" 'local parent="${RUNNER_TEMP:-/tmp}"'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-verify-runner-selftest.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-screenshotter-smoke.sh'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_TEST_IMAGE="${IMAGE_REGISTRY}/hostlet-screenshotter:${SHA_TAG}"'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_SKIP_BUILD=1'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-core-workflow-contracts.sh'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-verify-runner-selftest.sh'
assert_contains "${ROOT}/.github/workflows/release.yml" 'HOSTLET_SCREENSHOTTER_TEST_IMAGE="${IMAGE_REGISTRY}/hostlet-screenshotter:${SHA_TAG}"'
assert_contains "${ROOT}/.github/workflows/release.yml" 'HOSTLET_SCREENSHOTTER_SKIP_BUILD=1'
assert_contains "${CI_WORKFLOW}" 'runs-on: ubuntu-latest'
assert_contains "${STAGING_DEPLOYABILITY}" 'runs-on: ubuntu-latest'
assert_contains "${FULL_CI_WORKFLOW}" 'runs-on: [self-hosted, Linux, X64, hostlet-core]'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'TMP_DIR="$(ci_tmp_dir hostlet-self-api "${RUN_ID}")"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'TMP_DIR="$(ci_tmp_dir hostlet-self-deploy "${RUN_ID}")"'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'timed out waiting for self-hosted API'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'HOSTLET_SELF_HOSTED_AGENT_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'timed out waiting for self-hosted agent'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'ci_build_binary hostlet-api hostlet-api'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" '"$(ci_binary_path hostlet-api)"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ci_build_binary hostlet-api hostlet-api'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" '"$(ci_binary_path hostlet-api)"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ci_build_binary hostlet-agent hostlet-agent'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" '"$(ci_binary_path hostlet-agent)"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ensure_railpack()'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'scripts/ci-install-railpack.sh'
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

python3 - "${ROOT}/.github/workflows" <<'PY'
import sys
import re
from pathlib import Path

workflow_dir = Path(sys.argv[1])
violations = []
for workflow in workflow_dir.glob("*.yml"):
    text = workflow.read_text()
    if "pull_request:" not in text:
        continue
    if re.search(r"runs-on:\s*(?:\[.*self-hosted.*\]|self-hosted)", text):
        violations.append(workflow.name)

if violations:
    joined = ", ".join(sorted(violations))
    raise SystemExit(f"pull_request workflows must not use self-hosted runners: {joined}")
PY

python3 - "${STAGING_WORKFLOW}" "${ROOT}/.github/workflows/release.yml" <<'PY'
import sys
from pathlib import Path

staging = Path(sys.argv[1]).read_text()
release = Path(sys.argv[2]).read_text()

staging_smoke = staging.index("scripts/ci-screenshotter-smoke.sh")
staging_push = staging.index('docker push "${IMAGE_REGISTRY}/hostlet-${app}:staging"')
if staging_smoke > staging_push:
    raise SystemExit("staging workflow must smoke-test screenshotter before pushing it")

release_smoke = release.index("scripts/ci-screenshotter-smoke.sh")
release_push = release.index('docker push "${IMAGE_REGISTRY}/${image}:${RELEASE_TAG}"')
if release_smoke > release_push:
    raise SystemExit("release workflow must smoke-test screenshotter before pushing it")
PY

echo "core workflow contracts passed"
