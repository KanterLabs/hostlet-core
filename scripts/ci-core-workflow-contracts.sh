#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_WORKFLOW="${ROOT}/.github/workflows/staging.yml"
SELF_HOSTED_LIB="${ROOT}/scripts/ci-self-hosted-lib.sh"
CI_WORKFLOW="${ROOT}/.github/workflows/ci.yml"
PR_WORKFLOW="${ROOT}/.github/workflows/pr-homelab-ci.yml"
STAGING_DEPLOYABILITY="${ROOT}/.github/workflows/deployability.yml"
FULL_CI_WORKFLOW="${ROOT}/.github/workflows/full-ci.yml"
RELEASE_WORKFLOW="${ROOT}/.github/workflows/release.yml"
PREWARM_WORKFLOW="${ROOT}/.github/workflows/runner-fleet-prewarm.yml"
DATABASE_WORKFLOW="${ROOT}/.github/workflows/database-tests.yml"
ACTIONLINT_CONFIG="${ROOT}/.github/actionlint.yaml"
RELEASE_MAINLINE_GATE="${ROOT}/scripts/ci-release-mainline-gate.sh"
RELEASE_MAINLINE_SELFTEST="${ROOT}/scripts/ci-release-mainline-gate-selftest.sh"

assert_contains() {
  local file="$1"
  local needle="$2"
  if ! grep -Fq -- "${needle}" "${file}"; then
    echo "${file#${ROOT}/} missing expected text: ${needle}" >&2
    exit 1
  fi
}

assert_not_contains() {
  local file="$1"
  local needle="$2"
  if grep -Fq -- "${needle}" "${file}"; then
    echo "${file#${ROOT}/} contains forbidden text: ${needle}" >&2
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
assert_contains "${CI_WORKFLOW}" 'scripts/check-migration-versions.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-screenshotter-smoke.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-docker-retry.sh docker build'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_TEST_IMAGE="${IMAGE_REGISTRY}/hostlet-screenshotter:${SHA_TAG}"'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_SKIP_BUILD=1'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-docker-retry.sh docker build'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-docker-retry.sh docker push "${IMAGE_REGISTRY}/hostlet-${app}:staging"'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-core-workflow-contracts.sh'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-verify-runner-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'bash scripts/ci-db-tests-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'bash scripts/ci-release-mainline-gate-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${STAGING_WORKFLOW}" 'uses: ./.github/workflows/database-tests.yml'
assert_contains "${STAGING_WORKFLOW}" 'needs: [secrets, rust, database, web, topology-e2e, remote-build]'
assert_contains "${STAGING_WORKFLOW}" 'GHCR_PAT: ${{ secrets.GHCR_PAT }}'
assert_contains "${STAGING_WORKFLOW}" 'repos/KanterLabs/hostlet-cloud/dispatches'
assert_not_contains "${STAGING_WORKFLOW}" 'packages: write'
assert_not_contains "${STAGING_WORKFLOW}" 'actions/checkout@v4'
assert_not_contains "${STAGING_WORKFLOW}" 'dtolnay/rust-toolchain@stable'
assert_contains "${RELEASE_WORKFLOW}" 'docker buildx build --platform linux/amd64 --push'
assert_contains "${RELEASE_WORKFLOW}" 'HOSTLET_MAX_GLIBC_VERSION: "2.39"'
assert_contains "${RELEASE_WORKFLOW}" 'readelf --version-info target/release/hostlet'
assert_not_contains "${RELEASE_WORKFLOW}" 'linux/arm64'
assert_not_contains "${RELEASE_WORKFLOW}" 'hostlet-linux-arm64'
assert_contains "${RELEASE_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_TEST_IMAGE="${IMAGE_REGISTRY}/hostlet-screenshotter:${SHA_TAG}"'
assert_contains "${RELEASE_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_SKIP_BUILD=1'
assert_contains "${RELEASE_WORKFLOW}" 'uses: ./.github/workflows/database-tests.yml'
assert_contains "${RELEASE_WORKFLOW}" 'needs: [database, release-validation]'
assert_contains "${RELEASE_WORKFLOW}" 'GHCR_PAT: ${{ secrets.GHCR_PAT }}'
assert_contains "${RELEASE_WORKFLOW}" 'name: Require release source on main'
assert_contains "${RELEASE_WORKFLOW}" 'bash scripts/ci-release-mainline-gate.sh'
assert_contains "${RELEASE_WORKFLOW}" 'bash scripts/ci-release-mainline-gate-selftest.sh'
assert_contains "${RELEASE_WORKFLOW}" 'fetch-depth: 0'
assert_contains "${CI_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${CI_WORKFLOW}" 'uses: ./.github/workflows/database-tests.yml'
assert_contains "${CI_WORKFLOW}" 'bash scripts/ci-db-tests-selftest.sh'
assert_contains "${CI_WORKFLOW}" 'bash scripts/ci-release-mainline-gate-selftest.sh'
assert_not_contains "${CI_WORKFLOW}" 'pull_request:'
assert_contains "${PR_WORKFLOW}" 'pull_request_target:'
assert_contains "${PR_WORKFLOW}" "homelab-ci-approved"
assert_contains "${PR_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${PR_WORKFLOW}" 'persist-credentials: false'
assert_contains "${PR_WORKFLOW}" 'ref: ${{ github.event.pull_request.head.sha }}'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-verify-runner.sh'
assert_contains "${CI_WORKFLOW}" 'node --version && pnpm --version'
assert_contains "${CI_WORKFLOW}" 'CARGO_BUILD_JOBS: "4"'
assert_contains "${RELEASE_WORKFLOW}" 'CARGO_BUILD_JOBS: "4"'
assert_contains "${STAGING_WORKFLOW}" 'CARGO_BUILD_JOBS: "4"'
assert_contains "${FULL_CI_WORKFLOW}" 'CARGO_BUILD_JOBS: "4"'
assert_contains "${STAGING_DEPLOYABILITY}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${FULL_CI_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${PREWARM_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${ACTIONLINT_CONFIG}" 'homelab'
assert_contains "${ACTIONLINT_CONFIG}" 'homelab-heavy'
assert_not_contains "${ACTIONLINT_CONFIG}" 'hostlet-core-v2'
assert_contains "${FULL_CI_WORKFLOW}" "group: full-ci-\${{ github.event_name == 'schedule' && 'staging' || github.ref }}"
assert_contains "${STAGING_DEPLOYABILITY}" "group: deployability-\${{ github.event_name == 'schedule' && 'staging' || github.ref }}"
assert_contains "${DATABASE_WORKFLOW}" 'runs-on: homelab-heavy'
assert_contains "${DATABASE_WORKFLOW}" 'timeout-minutes: 20'
assert_contains "${DATABASE_WORKFLOW}" 'HOSTLET_DB_TEST_REQUIRED: "1"'
assert_contains "${DATABASE_WORKFLOW}" 'image: postgres:16-alpine@sha256:'
assert_contains "${DATABASE_WORKFLOW}" 'bash scripts/ci-db-tests.sh'
assert_contains "${DATABASE_WORKFLOW}" 'persist-credentials: false'
assert_contains "${ROOT}/scripts/ci-db-tests.sh" 'state::tests::required_database_preflight'
assert_contains "${ROOT}/scripts/ci-db-tests.sh" 'PGCONNECT_TIMEOUT'
assert_contains "${ROOT}/scripts/ci-db-tests.sh" 'statement_timeout=15000'
assert_contains "${ROOT}/scripts/ci-db-tests.sh" 'database != "hostlet_ci_test"'
assert_contains "${ROOT}/scripts/ci-db-tests.sh" 'target_routing_keys = {"host", "hostaddr", "port", "dbname"}'
assert_contains "${ROOT}/scripts/ci-db-tests.sh" 'safe_query_keys = {'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'remote-host'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'substring-testimonials'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'query-host-percent-encoded'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'query-dbname-percent-encoded'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'query-host-case-variant'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'query-duplicate-routing-key'
assert_contains "${ROOT}/scripts/ci-db-tests-selftest.sh" 'explicit-override-still-rejects-routing-query'
assert_contains "${RELEASE_MAINLINE_GATE}" "'+refs/heads/main:refs/remotes/origin/main'"
assert_contains "${RELEASE_MAINLINE_GATE}" 'git merge-base --is-ancestor'
assert_contains "${RELEASE_MAINLINE_GATE}" 'release publication requires an exact stable tag vX.Y.Z'
assert_contains "${RELEASE_MAINLINE_GATE}" '^v[0-9]+\.[0-9]+\.[0-9]+$'
assert_contains "${RELEASE_MAINLINE_GATE}" 'refs/heads/main'
assert_contains "${RELEASE_MAINLINE_SELFTEST}" 'arbitrary-side-tag'
assert_contains "${RELEASE_MAINLINE_SELFTEST}" 'prerelease-tag'
assert_contains "${RELEASE_MAINLINE_SELFTEST}" 'branch-dry-run'
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
assert_contains "${ROOT}/scripts/ci-install-railpack.sh" '${HOME}/.hostlet-core/railpack/${version}'
assert_contains "${ROOT}/scripts/ci-install-railpack.sh" '--version | grep -Fq "${version}"'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'docker info'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'mountpoint -q /var/lib/docker'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'HOSTLET_RUNNER_DOCKER_DISK_FAIL_PERCENT'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'HOSTLET_ALLOWED_RUNNER_PREFIX'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'HOSTLET_ALLOW_ARC_HOST_PATHS'
assert_contains "${ROOT}/scripts/ci-verify-runner.sh" 'forbidden_k8s_token_path'
assert_contains "${ROOT}/scripts/ci-verify-runner-selftest.sh" 'STUB_DOCKER_FAIL=1'
assert_contains "${ROOT}/scripts/ci-verify-runner-selftest.sh" 'HOSTLET_ALLOW_LOW_DOCKER_DISK=1'
assert_contains "${ROOT}/scripts/ci-verify-runner-selftest.sh" 'arc-host-path-exposed'
assert_not_contains "${ROOT}/scripts/backup.sh" '--single-transaction'
assert_not_contains "${ROOT}/apps/api/src/cleanup.rs" 'd.updated_at'

for workflow in \
  "${CI_WORKFLOW}" \
  "${PR_WORKFLOW}" \
  "${STAGING_WORKFLOW}" \
  "${STAGING_DEPLOYABILITY}" \
  "${FULL_CI_WORKFLOW}" \
  "${RELEASE_WORKFLOW}" \
  "${PREWARM_WORKFLOW}" \
  "${DATABASE_WORKFLOW}"; do
  assert_not_contains "${workflow}" 'runs-on: [self-hosted'
  assert_not_contains "${workflow}" 'hostlet-core-v2'
  assert_not_contains "${workflow}" 'runs-on: ubuntu-latest'
  assert_not_contains "${workflow}" 'actions/checkout@v4'
  assert_not_contains "${workflow}" 'dtolnay/rust-toolchain@stable'
done

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

python3 - \
  "${CI_WORKFLOW}" \
  "${STAGING_WORKFLOW}" \
  "${STAGING_DEPLOYABILITY}" \
  "${FULL_CI_WORKFLOW}" \
  "${PR_WORKFLOW}" \
  "${PREWARM_WORKFLOW}" \
  "${RELEASE_WORKFLOW}" \
  "${DATABASE_WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

expected = {
    sys.argv[1]: {
        "secrets": "homelab",
        "rust": "homelab-heavy",
        "web": "homelab-heavy",
        "compose": "homelab",
        "docker": "homelab-heavy",
    },
    sys.argv[2]: {
        "secrets": "homelab",
        "rust": "homelab-heavy",
        "web": "homelab-heavy",
        "topology-e2e": "homelab-heavy",
        "remote-build": "homelab-heavy",
        "images": "homelab-heavy",
        "notify-cloud": "homelab",
    },
    sys.argv[3]: {
        "generated-apps": "homelab-heavy",
        "self-hosted-api": "homelab-heavy",
        "patchwork-canary": "homelab-heavy",
    },
    sys.argv[4]: {
        "full-web-visual": "homelab-heavy",
        "full-self-hosted-deploy": "homelab-heavy",
        "full-compose-and-release": "homelab-heavy",
    },
    sys.argv[5]: {
        "clear-approval": "homelab",
        "secrets": "homelab",
        "rust": "homelab-heavy",
        "web": "homelab-heavy",
        "compose": "homelab",
        "docker": "homelab-heavy",
        "remote-build": "homelab-heavy",
    },
    sys.argv[6]: {"prewarm": "homelab"},
    sys.argv[7]: {
        "release-source": "homelab",
        "release-validation": "homelab-heavy",
        "linux-cli": "homelab-heavy",
    },
    sys.argv[8]: {"database": "homelab-heavy"},
}
for workflow_path, expected_runners in expected.items():
    workflow = Path(workflow_path).read_text()
    for job, runner in expected_runners.items():
        match = re.search(
            rf"^  {re.escape(job)}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        )
        if not match:
            raise SystemExit(f"{workflow_path} missing job {job}")
        if f"    runs-on: {runner}\n" not in match.group("body"):
            raise SystemExit(f"{workflow_path} job {job} must run on {runner}")
PY

python3 - "${PR_WORKFLOW}" <<'PY'
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
light_jobs = workflow.count("runs-on: homelab\n")
heavy_jobs = workflow.count("runs-on: homelab-heavy\n")
same_repo_guard = (
    "if: github.event.pull_request.head.repo.full_name == github.repository "
    "&& github.event.action == 'labeled' "
    "&& github.event.label.name == 'homelab-ci-approved'"
)
approved_guards = workflow.count(same_repo_guard)
if light_jobs != 3 or heavy_jobs != 4 or approved_guards != 6:
    raise SystemExit(
        "PR homelab CI must use canonical tiers behind same-repository approval: "
        f"light={light_jobs} heavy={heavy_jobs} guards={approved_guards}"
    )
PY

python3 - "${CI_WORKFLOW}" "${STAGING_WORKFLOW}" "${RELEASE_WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

for workflow_path in sys.argv[1:]:
    workflow = Path(workflow_path).read_text()
    match = re.search(
        r"^  database:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
        workflow,
        re.MULTILINE | re.DOTALL,
    )
    if not match:
        raise SystemExit(f"{workflow_path} missing unavoidable database job")
    body = match.group("body")
    if "    uses: ./.github/workflows/database-tests.yml\n" not in body:
        raise SystemExit(f"{workflow_path} database job must call reusable gate")
    if re.search(r"^    if:", body, re.MULTILINE):
        raise SystemExit(f"{workflow_path} database gate must not be conditional")
    if re.search(r"^    runs-on:", body, re.MULTILINE):
        raise SystemExit(f"{workflow_path} reusable database caller must not set runs-on")

release = Path(sys.argv[3]).read_text()
linux = re.search(
    r"^  linux-cli:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
    release,
    re.MULTILINE | re.DOTALL,
)
if not linux or "    needs: [database, release-validation]\n" not in linux.group("body"):
    raise SystemExit("release publisher must depend on database and release-validation")
linux_body = linux.group("body")
for permission in (
    "      contents: write\n",
    "      packages: write\n",
    "      id-token: write\n",
    "      attestations: write\n",
):
    if permission not in linux_body:
        raise SystemExit(f"release publisher missing permission: {permission.strip()}")

source = re.search(
    r"^  release-source:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
    release,
    re.MULTILINE | re.DOTALL,
)
if not source:
    raise SystemExit("release workflow missing release-source job")
source_body = source.group("body")
gate = "bash scripts/ci-release-mainline-gate.sh"
if gate not in source_body:
    raise SystemExit("release source job must enforce mainline tag ancestry")
if "      fetch-depth: 0\n" not in source_body:
    raise SystemExit("release source job must fetch history for ancestry validation")
if source_body.index(gate) > source_body.index("scripts/ci-verify-runner.sh"):
    raise SystemExit("release mainline gate must run before repository validation code")

for job in ("database", "release-validation"):
    match = re.search(
        rf"^  {job}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
        release,
        re.MULTILINE | re.DOTALL,
    )
    if not match or "    needs: [release-source]\n" not in match.group("body"):
        raise SystemExit(f"release {job} must wait for release-source")

release_header = release.split("\njobs:\n", 1)[0]
if "permissions:\n  contents: read\n" not in release_header:
    raise SystemExit("release workflow default permissions must be contents: read")
for permission in ("contents", "packages", "id-token", "attestations"):
    if f"  {permission}: write\n" in release_header:
        raise SystemExit(f"release workflow leaks top-level {permission}: write")

for job in ("release-source", "release-validation"):
    match = re.search(
        rf"^  {job}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
        release,
        re.MULTILINE | re.DOTALL,
    )
    if not match or "          persist-credentials: false\n" not in match.group("body"):
        raise SystemExit(f"release {job} checkout must not persist credentials")

if release.count("uses: actions/checkout@") != release.count(
    "persist-credentials: false"
):
    raise SystemExit("every release checkout must disable persisted credentials")
PY

python3 - "${STAGING_WORKFLOW}" "${DATABASE_WORKFLOW}" <<'PY'
import sys
from pathlib import Path

for workflow_path in sys.argv[1:]:
    workflow = Path(workflow_path).read_text()
    if workflow.count("uses: actions/checkout@") != workflow.count(
        "persist-credentials: false"
    ):
        raise SystemExit(
            f"{workflow_path}: every checkout must disable persisted credentials"
        )
PY

python3 - "${STAGING_WORKFLOW}" "${RELEASE_WORKFLOW}" <<'PY'
import sys
from pathlib import Path

staging = Path(sys.argv[1]).read_text()
release = Path(sys.argv[2]).read_text()

staging_smoke = staging.index("scripts/ci-screenshotter-smoke.sh")
staging_push = staging.index('scripts/ci-docker-retry.sh docker push "${IMAGE_REGISTRY}/hostlet-${app}:staging"')
if staging_smoke > staging_push:
    raise SystemExit("staging workflow must smoke-test screenshotter before pushing it")

# The release workflow publishes x86_64 images and smoke-tests the published
# SHA-tagged screenshotter before release assets are created.
release.index("scripts/ci-screenshotter-smoke.sh")
release.index("docker buildx build --platform linux/amd64 --push")
PY

echo "core workflow contracts passed"
