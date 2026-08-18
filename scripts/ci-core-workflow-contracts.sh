#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_WORKFLOW="${ROOT}/.github/workflows/staging.yml"
SELF_HOSTED_LIB="${ROOT}/scripts/ci-self-hosted-lib.sh"
CI_WORKFLOW="${ROOT}/.github/workflows/ci.yml"
PR_WORKFLOW="${ROOT}/.github/workflows/pr-homelab-ci.yml"
STAGING_PR_WORKFLOW="${ROOT}/.github/workflows/staging-pr.yml"
STAGING_PR_GATE="${ROOT}/scripts/ci-staging-pr-gate.py"
STAGING_PR_GATE_SELFTEST="${ROOT}/scripts/ci-staging-pr-gate-selftest.py"
STAGING_DEPLOYABILITY="${ROOT}/.github/workflows/deployability.yml"
FULL_CI_WORKFLOW="${ROOT}/.github/workflows/full-ci.yml"
RELEASE_WORKFLOW="${ROOT}/.github/workflows/release.yml"
RELEASE_CANDIDATE_WORKFLOW="${ROOT}/.github/workflows/release-candidate.yml"
PREWARM_WORKFLOW="${ROOT}/.github/workflows/runner-fleet-prewarm.yml"
DATABASE_WORKFLOW="${ROOT}/.github/workflows/database-tests.yml"
ACTIONLINT_CONFIG="${ROOT}/.github/actionlint.yaml"
RELEASE_MAINLINE_GATE="${ROOT}/scripts/ci-release-mainline-gate.sh"
RELEASE_MAINLINE_SELFTEST="${ROOT}/scripts/ci-release-mainline-gate-selftest.sh"
PREPARE_RELEASE_PR="${ROOT}/scripts/prepare-release-pr.sh"
PREPARE_RELEASE_PR_SELFTEST="${ROOT}/scripts/prepare-release-pr-selftest.sh"
SELF_HOSTED_LIB_SELFTEST="${ROOT}/scripts/ci-self-hosted-lib-selftest.sh"
RELEASE_EVIDENCE="${ROOT}/scripts/ci-release-evidence.py"
RELEASE_EVIDENCE_SELFTEST="${ROOT}/scripts/ci-release-evidence-selftest.py"

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
assert_contains "${CI_WORKFLOW}" 'scripts/ci-self-hosted-lib-selftest.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/check-migration-versions.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-screenshotter-smoke.sh'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-docker-retry.sh docker build'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_TEST_IMAGE="${IMAGE_REGISTRY}/hostlet-screenshotter:${SHA_TAG}"'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_SKIP_BUILD=1'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-docker-retry.sh docker build'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-docker-retry.sh docker push "${IMAGE_REGISTRY}/hostlet-${app}:staging"'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-core-workflow-contracts.sh'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-verify-runner-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'scripts/ci-self-hosted-lib-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'bash scripts/ci-db-tests-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'bash scripts/ci-release-mainline-gate-selftest.sh'
assert_contains "${STAGING_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${STAGING_WORKFLOW}" 'uses: ./.github/workflows/database-tests.yml'
assert_contains "${STAGING_WORKFLOW}" 'needs: [secrets, rust, database, web, topology-e2e, remote-build]'
assert_contains "${STAGING_WORKFLOW}" 'GHCR_PAT: ${{ secrets.GHCR_PAT }}'
assert_contains "${STAGING_WORKFLOW}" 'pin-cloud-staging:'
assert_contains "${STAGING_WORKFLOW}" 'needs: [images]'
assert_contains "${STAGING_WORKFLOW}" 'repository: KanterLabs/hostlet-cloud'
assert_contains "${STAGING_WORKFLOW}" 'ref: staging'
assert_contains "${STAGING_WORKFLOW}" 'token: ${{ secrets.CLOUD_DISPATCH_PAT }}'
assert_contains "${STAGING_WORKFLOW}" 'GH_TOKEN: ${{ secrets.CLOUD_DISPATCH_PAT }}'
assert_contains "${STAGING_WORKFLOW}" 'path: hostlet-cloud'
assert_contains "${STAGING_WORKFLOW}" 'working-directory: hostlet-cloud'
assert_contains "${STAGING_WORKFLOW}" 'scripts/update-core-staging-pin.sh "${GITHUB_SHA}"'
assert_not_contains "${STAGING_WORKFLOW}" 'repository_dispatch'
assert_not_contains "${STAGING_WORKFLOW}" '/dispatches'
assert_not_contains "${STAGING_WORKFLOW}" 'core-staging-updated'
assert_not_contains "${STAGING_WORKFLOW}" 'core-drift-reviewed'
assert_not_contains "${STAGING_WORKFLOW}" 'auto-merge'
assert_not_contains "${STAGING_WORKFLOW}" 'uses: ./.github/workflows/release-candidate.yml'
assert_not_contains "${STAGING_WORKFLOW}" 'packages: write'
assert_not_contains "${STAGING_WORKFLOW}" 'actions/checkout@v4'
assert_not_contains "${STAGING_WORKFLOW}" 'dtolnay/rust-toolchain@stable'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'workflow_call:'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'workflow_dispatch:'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'runs-on: homelab'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'candidate-tests:'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'candidate-artifacts:'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'seal-candidate:'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'needs: [verify-staging]'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'needs: [verify-staging, candidate-tests, candidate-artifacts]'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'HOSTLET_RELEASE_CANDIDATE_MAX_AGE_SECONDS: "14400"'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'cargo build --release -p hostlet'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'readelf --version-info target/release/hostlet'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'Capture four existing staging image digests'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'schema": "hostlet.core.release-candidate/v1"'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" '"core": {'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" '"artifacts": {'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" '--expected-sha'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" '--expected-tree'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" '--expected-version'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'name: core-release-candidate'
assert_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'retention-days: 14'
assert_not_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'certify-candidate:'
assert_not_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'candidate_sha":'
assert_not_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'docker build'
assert_not_contains "${RELEASE_CANDIDATE_WORKFLOW}" 'cargo test'
assert_contains "${RELEASE_WORKFLOW}" 'candidate_max_age_seconds:'
assert_contains "${RELEASE_WORKFLOW}" 'Publish ${{ inputs.release_version || github.ref_name }} from'
assert_contains "${RELEASE_WORKFLOW}" '${{ inputs.candidate_sha || github.sha }}'
assert_contains "${RELEASE_WORKFLOW}" 'default: "14400"'
assert_contains "${RELEASE_WORKFLOW}" 'emergency_acknowledgement:'
assert_contains "${RELEASE_WORKFLOW}" 'I_UNDERSTAND_LEGACY_RELEASE'
assert_contains "${RELEASE_WORKFLOW}" 'release-candidate.py validate'
assert_contains "${RELEASE_WORKFLOW}" 'docker buildx imagetools create --tag'
assert_contains "${RELEASE_WORKFLOW}" 'name: candidate-publication'
assert_contains "${RELEASE_WORKFLOW}" '"publication": {'
assert_contains "${RELEASE_WORKFLOW}" '"run_id": int(os.environ["GITHUB_RUN_ID"])'
assert_contains "${RELEASE_WORKFLOW}" '"run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"])'
assert_contains "${RELEASE_WORKFLOW}" '"workflow": "release.yml"'
assert_contains "${RELEASE_WORKFLOW}" 'name: Require release source on main'
assert_contains "${RELEASE_WORKFLOW}" 'bash scripts/ci-release-mainline-gate.sh'
assert_contains "${RELEASE_WORKFLOW}" 'bash scripts/ci-release-mainline-gate-selftest.sh'
assert_contains "${RELEASE_WORKFLOW}" 'fetch-depth: 0'
assert_contains "${RELEASE_WORKFLOW}" 'docker buildx build --platform linux/amd64 --push'
assert_contains "${RELEASE_WORKFLOW}" 'HOSTLET_SCREENSHOTTER_SKIP_BUILD=1'
assert_contains "${RELEASE_WORKFLOW}" 'uses: ./.github/workflows/database-tests.yml'
assert_contains "${RELEASE_WORKFLOW}" 'needs: [release-source, database, release-validation]'
assert_contains "${RELEASE_WORKFLOW}" 'GHCR_PAT: ${{ secrets.GHCR_PAT }}'
assert_not_contains "${RELEASE_WORKFLOW}" 'receipt["candidate_sha"]'
assert_not_contains "${RELEASE_WORKFLOW}" 'image.get("digest"'
python3 - "${RELEASE_WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
for action in re.findall(r"^\s+uses:\s+([^\s]+)$", workflow, re.MULTILINE):
    if action.startswith("./"):
        continue
    if "@" not in action or not re.fullmatch(r"[0-9a-f]{40}", action.rsplit("@", 1)[1]):
        raise SystemExit(f"release action is not full-SHA pinned: {action}")
PY
assert_contains "${CI_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${CI_WORKFLOW}" 'uses: ./.github/workflows/database-tests.yml'
assert_contains "${CI_WORKFLOW}" 'bash scripts/ci-db-tests-selftest.sh'
assert_contains "${CI_WORKFLOW}" 'bash scripts/ci-release-mainline-gate-selftest.sh'
assert_not_contains "${CI_WORKFLOW}" 'pull_request:'
assert_contains "${PR_WORKFLOW}" 'pull_request_target:'
assert_contains "${PR_WORKFLOW}" 'scripts/ci-self-hosted-lib-selftest.sh'
assert_contains "${PR_WORKFLOW}" "homelab-ci-approved"
assert_contains "${PR_WORKFLOW}" 'bash scripts/ci-backup-selftest.sh'
assert_contains "${PR_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${PR_WORKFLOW}" 'persist-credentials: false'
assert_contains "${PR_WORKFLOW}" 'ref: ${{ github.event.pull_request.head.sha }}'
assert_contains "${STAGING_PR_WORKFLOW}" 'pull_request:'
assert_not_contains "${STAGING_PR_WORKFLOW}" 'pull_request_target:'
assert_contains "${STAGING_PR_WORKFLOW}" 'scripts/ci-self-hosted-lib-selftest.sh'
assert_contains "${STAGING_PR_WORKFLOW}" '`pull_request` YAML is PR-controlled'
assert_contains "${STAGING_PR_WORKFLOW}" 'writers who can label are staging deploy admins'
assert_contains "${STAGING_PR_WORKFLOW}" 'fork PRs fail the same-repo'
assert_contains "${STAGING_PR_WORKFLOW}" 'not eliminate this boundary without default-branch workflow ownership'
assert_contains "${STAGING_PR_WORKFLOW}" 'the evaluator is not present on its base SHA yet'
assert_contains "${STAGING_PR_WORKFLOW}" 'HOSTLET_STAGING_PR_APPROVAL_LABEL: staging-homelab-ci-approved'
assert_contains "${STAGING_PR_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${STAGING_PR_WORKFLOW}" 'group: staging-pr-${{ github.event.pull_request.number }}'
assert_contains "${STAGING_PR_WORKFLOW}" 'ref: ${{ github.event.pull_request.head.sha }}'
assert_contains "${STAGING_PR_WORKFLOW}" 'persist-credentials: false'
assert_contains "${STAGING_PR_GATE}" 'REVOCATION_DEPENDENCY = "revoke-approval-on-update"'
assert_contains "${STAGING_PR_GATE}" 'unexpected = sorted(set(payload) - {*DEPENDENCIES, REVOCATION_DEPENDENCY})'
assert_contains "${STAGING_PR_GATE}" 'if result != "success"'
assert_contains "${STAGING_PR_GATE}" 'event_base = require_sha(required_env(environment, "EVENT_BASE_SHA"), "event base")'
assert_contains "${STAGING_PR_GATE}" 'live_base = require_sha(nested(pull, "base", "sha"), "live pull-request base")'
assert_contains "${STAGING_PR_GATE}" 'if live_base != event_base:'
assert_contains "${STAGING_PR_GATE}" 'return validate_live_pull(fetch_live_pull(environment), environment)'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'for dependency in gate.DEPENDENCIES:'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'for conclusion in ("skipped", "neutral", "failure", "cancelled")'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'f"missing-{dependency}"'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'unexpected-dependency'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'stale-live-base'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'malformed-live-base'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'missing-live-base'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'stale-live-head'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'missing-approval'
assert_contains "${STAGING_PR_GATE_SELFTEST}" 'api-failure'
assert_contains "${CI_WORKFLOW}" 'scripts/ci-verify-runner.sh'
assert_contains "${CI_WORKFLOW}" 'node --version && pnpm --version'
assert_contains "${CI_WORKFLOW}" 'CARGO_BUILD_JOBS: "8"'
assert_contains "${RELEASE_WORKFLOW}" 'CARGO_BUILD_JOBS: "8"'
assert_contains "${STAGING_WORKFLOW}" 'CARGO_BUILD_JOBS: "8"'
assert_contains "${FULL_CI_WORKFLOW}" 'CARGO_BUILD_JOBS: "8"'
assert_contains "${STAGING_DEPLOYABILITY}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${FULL_CI_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${PREWARM_WORKFLOW}" 'HOSTLET_ALLOWED_RUNNER_PREFIX: homelab-'
assert_contains "${ACTIONLINT_CONFIG}" 'homelab'
assert_contains "${ACTIONLINT_CONFIG}" 'homelab-heavy'
assert_not_contains "${ACTIONLINT_CONFIG}" 'hostlet-core-v2'
assert_contains "${FULL_CI_WORKFLOW}" "group: full-ci-\${{ github.event_name == 'schedule' && 'staging' || github.ref }}"
assert_contains "${STAGING_DEPLOYABILITY}" "group: deployability-\${{ github.event_name == 'schedule' && 'staging' || github.ref }}"
assert_contains "${DATABASE_WORKFLOW}" 'runs-on: homelab-heavy'
assert_contains "${DATABASE_WORKFLOW}" 'CARGO_BUILD_JOBS: "2"'
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
assert_contains "${PREPARE_RELEASE_PR}" 'release-candidate/v${version}'
assert_contains "${PREPARE_RELEASE_PR}" 'pulls?state=all&head='
assert_contains "${PREPARE_RELEASE_PR}" 'baseSha'
assert_contains "${PREPARE_RELEASE_PR}" 'headSha'
assert_contains "${PREPARE_RELEASE_PR}" 'Do not merge'
assert_contains "${PREPARE_RELEASE_PR}" 'origin/${source_branch}'
assert_contains "${PREPARE_RELEASE_PR}" 'refs/tags/v${version}'
assert_contains "${PREPARE_RELEASE_PR_SELFTEST}" 'base mismatch was accepted'
assert_contains "${PREPARE_RELEASE_PR_SELFTEST}" 'branch collision was accepted'
assert_contains "${PREPARE_RELEASE_PR_SELFTEST}" 'version mismatch was accepted'
assert_contains "${PREPARE_RELEASE_PR_SELFTEST}" 'tag collision was accepted'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'TMP_DIR="$(ci_tmp_dir hostlet-self-api "${RUN_ID}")"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'TMP_DIR="$(ci_tmp_dir hostlet-self-deploy "${RUN_ID}")"'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'timed out waiting for self-hosted API'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'HOSTLET_SELF_HOSTED_AGENT_ATTEMPTS:-300'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'timed out waiting for self-hosted agent'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'ci_build_binary hostlet-api hostlet-api'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" '"$(ci_binary_path hostlet-api)"'
assert_contains "${ROOT}/scripts/ci-self-hosted-api-smoke.sh" 'PostgreSQL readiness failed; aborting self-hosted API smoke before API startup'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ci_build_binary hostlet-api hostlet-api'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" '"$(ci_binary_path hostlet-api)"'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'PostgreSQL readiness failed; aborting self-hosted deploy E2E before registry startup'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" 'ci_build_binary hostlet-agent hostlet-agent'
assert_contains "${ROOT}/scripts/ci-self-hosted-deploy-e2e.sh" '"$(ci_binary_path hostlet-agent)"'
assert_contains "${ROOT}/scripts/ci-remote-build-e2e.sh" 'PostgreSQL readiness failed; aborting remote-build E2E before registry startup'
assert_contains "${ROOT}/scripts/ci-self-hosted-lib.sh" 'Timed out waiting for PostgreSQL readiness after 60 attempts'
assert_contains "${ROOT}/scripts/ci-self-hosted-lib.sh" 'return 1'
assert_contains "${SELF_HOSTED_LIB_SELFTEST}" 'all-probes-fail'
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

PYTHONDONTWRITEBYTECODE=1 python3 "${STAGING_PR_GATE_SELFTEST}"
bash "${PREPARE_RELEASE_PR_SELFTEST}"

for workflow in \
  "${CI_WORKFLOW}" \
  "${PR_WORKFLOW}" \
  "${STAGING_PR_WORKFLOW}" \
  "${STAGING_WORKFLOW}" \
  "${STAGING_DEPLOYABILITY}" \
  "${FULL_CI_WORKFLOW}" \
  "${RELEASE_WORKFLOW}" \
  "${RELEASE_CANDIDATE_WORKFLOW}" \
  "${PREWARM_WORKFLOW}" \
  "${DATABASE_WORKFLOW}"; do
  assert_not_contains "${workflow}" 'runs-on: [self-hosted'
  assert_not_contains "${workflow}" 'hostlet-core-v2'
  assert_not_contains "${workflow}" 'runs-on: ubuntu-latest'
  assert_not_contains "${workflow}" 'actions/checkout@v4'
  assert_not_contains "${workflow}" 'dtolnay/rust-toolchain@stable'
done

python3 - \
  "${CI_WORKFLOW}" \
  "${STAGING_WORKFLOW}" \
  "${STAGING_DEPLOYABILITY}" \
  "${FULL_CI_WORKFLOW}" \
  "${PR_WORKFLOW}" \
  "${STAGING_PR_WORKFLOW}" \
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
        "pin-cloud-staging": "homelab",
    },
    sys.argv[3]: {
        "generated-apps": "homelab-heavy",
        "self-hosted-api": "homelab",
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
    sys.argv[6]: {
        "revoke-approval-on-update": "homelab",
        "secrets": "homelab",
        "rust": "homelab-heavy",
        "database": "homelab-heavy",
        "web": "homelab-heavy",
        "compose": "homelab",
        "docker": "homelab-heavy",
        "topology-e2e": "homelab-heavy",
        "remote-build": "homelab-heavy",
        "staging-pr-gate": "homelab",
    },
    sys.argv[7]: {"prewarm": "homelab"},
    sys.argv[8]: {
        "release-source": "homelab",
        "candidate-publication": "homelab",
        "release-validation": "homelab-heavy",
        "linux-cli": "homelab-heavy",
    },
    sys.argv[9]: {"database": "homelab-heavy"},
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

python3 - "${RELEASE_CANDIDATE_WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
expected = {
    "verify-staging": "homelab",
    "candidate-tests": "homelab-heavy",
    "candidate-artifacts": "homelab-heavy",
    "seal-candidate": "homelab",
}
for job, runner in expected.items():
    match = re.search(
        rf"^  {re.escape(job)}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
        workflow,
        re.MULTILINE | re.DOTALL,
    )
    if not match:
        raise SystemExit(f"{sys.argv[1]} missing job {job}")
    if f"    runs-on: {runner}\n" not in match.group("body"):
        raise SystemExit(f"{job} must run on {runner}")

for action in re.findall(r"^\s+uses:\s+([^\s]+)$", workflow, re.MULTILINE):
    if action.startswith("./"):
        continue
    if "@" not in action or not re.fullmatch(r"[0-9a-f]{40}", action.rsplit("@", 1)[1]):
        raise SystemExit(f"release candidate action is not full-SHA pinned: {action}")

verify = re.search(r"^  verify-staging:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)", workflow, re.MULTILINE | re.DOTALL).group("body")
tests = re.search(r"^  candidate-tests:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)", workflow, re.MULTILINE | re.DOTALL).group("body")
artifacts = re.search(r"^  candidate-artifacts:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)", workflow, re.MULTILINE | re.DOTALL).group("body")
seal = re.search(r"^  seal-candidate:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)", workflow, re.MULTILINE | re.DOTALL).group("body")
if "staging_sha" not in verify or "staging_run_id" not in verify:
    raise SystemExit("verify-staging must accept exact SHA and run inputs")
if "needs: [verify-staging]" not in tests or "needs: [verify-staging]" not in artifacts:
    raise SystemExit("candidate tests/artifacts must run in parallel after verify-staging")
if "needs: [verify-staging, candidate-tests, candidate-artifacts]" not in seal:
    raise SystemExit("seal-candidate must wait for both parallel lanes")
if "cargo build --release -p hostlet" not in artifacts or "cargo build --release -p hostlet" in tests:
    raise SystemExit("only candidate-artifacts may build the CLI")
if "scripts/ci-self-hosted-api-smoke.sh" not in tests or "scripts/ci-self-hosted-deploy-e2e.sh" not in tests:
    raise SystemExit("candidate-tests must run API and deploy E2E")
if "scripts/ci-install-railpack.sh" not in tests:
    raise SystemExit("candidate-tests must cover Railpack")
if "--expected-sha" not in seal or "--expected-tree" not in seal or "--expected-version" not in seal:
    raise SystemExit("seal-candidate must bind all expected receipt identities")
if 'name: core-release-candidate' not in seal or 'retention-days: 14' not in seal:
    raise SystemExit("seal-candidate must upload one durable receipt artifact")
for marker in (
    '"schema": "hostlet.core.release-candidate/v1"',
    '"core": {',
    '"version":',
    '"workflows": {',
    '"created_at":',
    '"expires_at":',
    '"images": images',
    '"artifacts": {',
):
    if marker not in seal:
        raise SystemExit(f"sealed receipt draft is missing {marker}")
if "name: release-candidate-tests" not in seal or "name: release-candidate-artifacts" not in seal:
    raise SystemExit("seal-candidate must download both parallel evidence artifacts")
if "14400" not in workflow or "604800" in workflow:
    raise SystemExit("Core candidate freshness must be exactly four hours")
PY

python3 - "${PR_WORKFLOW}" <<'PY'
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
light_jobs = workflow.count("runs-on: homelab\n")
heavy_jobs = workflow.count("runs-on: homelab-heavy\n")
approval_guards = workflow.count("github.event.label.name == 'homelab-ci-approved'")
release_exclusions = workflow.count("!startsWith(github.event.pull_request.head.ref, 'release-candidate/v')")
if light_jobs != 4 or heavy_jobs != 4 or approval_guards != 6 or release_exclusions != 6:
    raise SystemExit(
        "PR homelab CI must use canonical tiers behind same-repository approval "
        "and exclude release candidates from heavy lanes: "
        f"light={light_jobs} heavy={heavy_jobs} approvals={approval_guards} exclusions={release_exclusions}"
    )
PY

python3 - "${STAGING_PR_WORKFLOW}" "${STAGING_WORKFLOW}" "${STAGING_PR_GATE}" <<'PY'
import re
import sys
from pathlib import Path

pr_path = Path(sys.argv[1])
staging_path = Path(sys.argv[2])
workflow = pr_path.read_text()
staging = staging_path.read_text()
gate_helper = Path(sys.argv[3]).read_text()


def job_body(source: str, name: str) -> str:
    match = re.search(
        rf"^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
        source,
        re.MULTILINE | re.DOTALL,
    )
    if not match:
        raise SystemExit(f"{pr_path} missing job {name}")
    return match.group("body")


header = workflow.split("\njobs:\n", 1)[0]
if "permissions:\n  contents: read\n" not in header:
    raise SystemExit("staging PR workflow default permissions must be contents: read")
if "pull_request_target:" in header:
    raise SystemExit("staging PR workflow must not use default-branch pull_request_target")

branch_match = re.search(r"^    branches:\n(?P<body>(?:      - .+\n)+)", header, re.MULTILINE)
if not branch_match or branch_match.group("body").split() != ["-", "staging"]:
    raise SystemExit("staging PR workflow must target only the staging base branch")

type_match = re.search(r"^    types:\n(?P<body>(?:      - .+\n)+)", header, re.MULTILINE)
expected_types = ["opened", "reopened", "synchronize", "labeled", "unlabeled"]
if not type_match:
    raise SystemExit("staging PR workflow is missing explicit activity types")
actual_types = [line.removeprefix("      - ") for line in type_match.group("body").splitlines()]
if actual_types != expected_types:
    raise SystemExit(f"staging PR workflow activity types changed: {actual_types}")

approval = "staging-homelab-ci-approved"
approved_guard = (
    "    if: github.event.pull_request.head.repo.full_name == github.repository "
    "&& github.event.action == 'labeled' "
    f"&& github.event.label.name == '{approval}'\n"
)
substantive_jobs = (
    "secrets",
    "rust",
    "database",
    "web",
    "compose",
    "docker",
    "topology-e2e",
    "remote-build",
)
for name in substantive_jobs:
    body = job_body(workflow, name)
    if approved_guard not in body:
        raise SystemExit(f"staging PR job {name} is not gated by fresh same-repo approval")
    for needle in (
        "repository: ${{ github.event.pull_request.head.repo.full_name }}",
        "ref: ${{ github.event.pull_request.head.sha }}",
        "persist-credentials: false",
        "scripts/ci-verify-runner.sh",
    ):
        if needle not in body:
            raise SystemExit(f"staging PR job {name} missing {needle}")

dependency_match = re.search(
    r'^DEPENDENCIES = \(\n(?P<body>.*?)^\)\n',
    gate_helper,
    re.MULTILINE | re.DOTALL,
)
if not dependency_match:
    raise SystemExit("staging PR aggregate helper dependency tuple is malformed")
helper_dependencies = tuple(
    re.findall(r'^    "([a-z0-9-]+)",$', dependency_match.group("body"), re.MULTILINE)
)
if helper_dependencies != substantive_jobs:
    raise SystemExit(
        f"staging PR helper dependencies changed: {helper_dependencies}"
    )

database = job_body(workflow, "database")
for needle in (
    "    timeout-minutes: 20\n",
    '      CARGO_BUILD_JOBS: "2"\n',
    '      HOSTLET_DB_TEST_REQUIRED: "1"\n',
    "image: postgres:16-alpine@sha256:57c72fd2a128e416c7fcc499958864df5301e940bca0a56f58fddf30ffc07777",
    "POSTGRES_DB: hostlet_ci_test",
    "postgresql://hostlet_ci:hostlet-ci-test-password@127.0.0.1:${{ job.services.postgres.ports['5432'] }}/hostlet_ci_test",
    "run: bash scripts/ci-db-tests.sh",
):
    if needle not in database:
        raise SystemExit(f"staging PR database lane missing {needle.strip()}")
if "uses: ./.github/workflows/database-tests.yml" in database:
    raise SystemExit("staging PR database lane must test the explicit PR head directly")

topology = job_body(workflow, "topology-e2e")
for needle in (
    '      CARGO_BUILD_JOBS: "8"\n',
    '      HOSTLET_E2E_SKIP_RAILPACK_FIXTURES: "1"\n',
    "scripts/ci-install-railpack.sh",
    "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8",
    "scripts/ci-self-hosted-deploy-e2e.sh",
):
    if needle not in topology:
        raise SystemExit(f"staging PR topology lane missing {needle.strip()}")
if topology.index("scripts/ci-install-railpack.sh") > topology.index(
    "scripts/ci-self-hosted-deploy-e2e.sh"
):
    raise SystemExit("staging PR topology lane must install Railpack before deploy E2E")

revoke = job_body(workflow, "revoke-approval-on-update")
for needle in (
    "github.event.pull_request.head.repo.full_name == github.repository",
    "github.event.action == 'synchronize'",
    "github.event.action == 'reopened'",
    f"contains(github.event.pull_request.labels.*.name, '{approval}')",
    "      pull-requests: write\n",
    "200|404)",
    "failed to revoke stale staging PR approval",
):
    if needle not in revoke:
        raise SystemExit(f"staging PR approval revoker missing {needle.strip()}")
if "actions/checkout@" in revoke:
    raise SystemExit("staging PR approval revoker must remain metadata-only")

gate = job_body(workflow, "staging-pr-gate")
for needle in (
    "    name: staging-pr-gate\n",
    "    if: always()\n",
    "    needs: [revoke-approval-on-update, secrets, rust, database, web, compose, docker, topology-e2e, remote-build]\n",
    "      contents: read\n",
    "      pull-requests: read\n",
    "repository: ${{ github.repository }}",
    "ref: ${{ github.event.pull_request.base.sha }}",
    "path: .hostlet-staging-pr-gate",
    "sparse-checkout: /scripts/ci-staging-pr-gate.py",
    "sparse-checkout-cone-mode: false",
    "persist-credentials: false",
    "GH_TOKEN: ${{ github.token }}",
    "PR_NUMBER: ${{ github.event.pull_request.number }}",
    "EVENT_ACTION: ${{ github.event.action }}",
    "EVENT_LABEL: ${{ github.event.label.name }}",
    "EVENT_HEAD_SHA: ${{ github.event.pull_request.head.sha }}",
    "EVENT_BASE_SHA: ${{ github.event.pull_request.base.sha }}",
    "HOSTLET_STAGING_PR_RESULTS_JSON: ${{ toJSON(needs) }}",
    "run: python3 .hostlet-staging-pr-gate/scripts/ci-staging-pr-gate.py",
):
    if needle not in gate:
        raise SystemExit(f"staging PR aggregate gate missing {needle.strip()}")
checkout_actions = re.findall(r"^\s+uses:\s*(.+)$", gate, re.MULTILINE)
if checkout_actions != ["actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5"]:
    raise SystemExit("staging PR aggregate gate must use only the pinned checkout action")
run_commands = re.findall(r"^\s+run:\s*(.+)$", gate, re.MULTILINE)
if run_commands != ["python3 .hostlet-staging-pr-gate/scripts/ci-staging-pr-gate.py"]:
    raise SystemExit("staging PR aggregate gate must execute only the base-owned helper")
for forbidden in (
    "ref: ${{ github.event.pull_request.head.sha }}",
    "ref: ${{ github.sha }}",
    "github.event.pull_request.merge_commit_sha",
    "continue-on-error",
):
    if forbidden in gate:
        raise SystemExit(f"staging PR aggregate gate contains unsafe behavior: {forbidden}")
if gate.count("sparse-checkout:") != 1 or gate.count("persist-credentials: false") != 1:
    raise SystemExit("staging PR aggregate gate checkout must remain base-owned and sparse")

if workflow.count("uses: actions/checkout@") != workflow.count("persist-credentials: false"):
    raise SystemExit("every staging PR checkout must disable persisted credentials")

dispatch_guard = (
    "    if: github.event_name != 'workflow_dispatch' "
    "|| github.ref == 'refs/heads/staging'\n"
)
staging_job_section = staging.split("\njobs:\n", 1)[1]
staging_jobs = re.findall(
    r"^  ([a-zA-Z0-9_-]+):\n",
    staging_job_section,
    re.MULTILINE,
)
for name in staging_jobs:
    if dispatch_guard not in job_body(staging_job_section, name):
        raise SystemExit(f"Core staging job {name} lacks the staging dispatch guard")

pin = job_body(staging, "pin-cloud-staging")
for needle in (
    "    needs: [images]\n",
    "repository: KanterLabs/hostlet-cloud",
    "ref: staging",
    "token: ${{ secrets.CLOUD_DISPATCH_PAT }}",
    "fetch-depth: 0",
    "submodules: recursive",
    "persist-credentials: false",
    "working-directory: hostlet-cloud",
    "GH_TOKEN: ${{ secrets.CLOUD_DISPATCH_PAT }}",
    'scripts/update-core-staging-pin.sh "${GITHUB_SHA}"',
):
    if needle not in pin:
        raise SystemExit(f"Core staging pin-PR producer missing {needle.strip()}")
for forbidden in (
    "repository_dispatch",
    "/dispatches",
    "core-drift-reviewed",
    "auto-merge",
):
    if forbidden in staging:
        raise SystemExit(f"Core staging workflow contains forbidden pin behavior: {forbidden}")
if staging.count("secrets.CLOUD_DISPATCH_PAT") != 2:
    raise SystemExit("Core staging token must be scoped only to Cloud checkout and pin helper")
PY

python3 - "${CI_WORKFLOW}" "${STAGING_WORKFLOW}" "${RELEASE_WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

staging_path = Path(sys.argv[2])
release_path = Path(sys.argv[3])
for workflow_path in sys.argv[1:]:
    path = Path(workflow_path)
    workflow = path.read_text()
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
    conditional = re.search(r"^    if:", body, re.MULTILINE)
    if path == staging_path:
        expected = (
            "    if: github.event_name != 'workflow_dispatch' "
            "|| github.ref == 'refs/heads/staging'\n"
        )
        if expected not in body:
            raise SystemExit("staging database gate must reject non-staging dispatches")
    elif path == release_path:
        expected = (
            "    if: github.event_name == 'workflow_dispatch' && inputs.release_mode == 'legacy' "
            "&& inputs.emergency_acknowledgement == 'I_UNDERSTAND_LEGACY_RELEASE'\n"
        )
        if expected not in body:
            raise SystemExit("release database gate must be emergency-acknowledged only")
    elif path.name == "ci.yml":
        expected = "    if: needs.release-evidence.outputs.reuse_heavy != 'true'\n"
        if expected not in body or "    needs: [release-evidence]\n" not in body:
            raise SystemExit("main CI database gate must wait for release evidence")
    elif conditional:
        raise SystemExit(f"{workflow_path} database gate must not be conditional")
    if re.search(r"^    runs-on:", body, re.MULTILINE):
        raise SystemExit(f"{workflow_path} reusable database caller must not set runs-on")

release = Path(sys.argv[3]).read_text()
linux = re.search(
    r"^  linux-cli:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
    release,
    re.MULTILINE | re.DOTALL,
)
if not linux or "    needs: [release-source, database, release-validation]\n" not in linux.group("body"):
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

candidate = re.search(
    r"^  candidate-publication:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
    release,
    re.MULTILINE | re.DOTALL,
)
if not candidate:
    raise SystemExit("release workflow missing candidate-publication")
candidate_body = candidate.group("body")
for forbidden in (
    "cargo build",
    "cargo test",
    "docker build ",
    "docker buildx build",
    "ci-self-hosted-api-smoke",
    "ci-self-hosted-deploy-e2e",
    "ci-install-railpack",
):
    if forbidden in candidate_body:
        raise SystemExit(f"normal candidate path must not contain {forbidden}")
for required in (
    "release-candidate.py validate",
    "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093",
    "docker buildx imagetools create --tag",
    "softprops/action-gh-release@3bb12739c298aeb8a4eeaf626c5b8d85266b0e65",
    "receipt[\"core\"][\"commit_sha\"]",
    "receipt[\"images\"]",
):
    if required not in candidate_body:
        raise SystemExit(f"normal candidate publication missing {required}")
if "inputs.release_mode == 'candidate'" not in candidate_body:
    raise SystemExit("candidate publication must be candidate-mode only")
legacy_jobs = ("database", "release-validation", "linux-cli")
for name in legacy_jobs:
    legacy = re.search(
        rf"^  {name}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
        release,
        re.MULTILINE | re.DOTALL,
    )
    if not legacy or "github.event_name == 'workflow_dispatch'" not in legacy.group("body"):
        raise SystemExit(f"legacy job {name} is not workflow-dispatch isolated")
    if "I_UNDERSTAND_LEGACY_RELEASE" not in legacy.group("body"):
        raise SystemExit(f"legacy job {name} lacks emergency acknowledgement")

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

python3 - "${STAGING_WORKFLOW}" "${STAGING_PR_WORKFLOW}" "${DATABASE_WORKFLOW}" <<'PY'
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

# The normal candidate publication only aliases immutable digests and reuses
# candidate bytes.  The old image builder/smoke test remains present solely in
# the explicitly acknowledged legacy job.
candidate = release.split("  candidate-publication:\n", 1)[1].split("\n  # Explicitly acknowledged emergency fallback.", 1)[0]
for forbidden in ("docker build ", "docker buildx build", "cargo build", "cargo test", "scripts/ci-screenshotter-smoke.sh"):
    if forbidden in candidate:
        raise SystemExit(f"normal candidate publication contains forbidden work: {forbidden}")
release.index("docker buildx build --platform linux/amd64 --push")
release.index("scripts/ci-screenshotter-smoke.sh")
PY

assert_contains "${PR_WORKFLOW}" 'name: release-candidate-gate'
assert_contains "${PR_WORKFLOW}" 'if: always()'
assert_contains "${PR_WORKFLOW}" 'ref: ${{ github.event.pull_request.base.sha }}'
assert_contains "${PR_WORKFLOW}" 'sparse-checkout-cone-mode: false'
assert_contains "${PR_WORKFLOW}" 'HOSTLET_PR_RESULTS_JSON: ${{ toJSON(needs) }}'
assert_contains "${PR_WORKFLOW}" 'HOSTLET_RELEASE_EVIDENCE_MODE: pr'
assert_contains "${PR_WORKFLOW}" 'EVENT_HEAD_SHA: ${{ github.event.pull_request.head.sha }}'
assert_contains "${PR_WORKFLOW}" 'run: python3 .hostlet-release-evidence/scripts/ci-release-evidence.py'
assert_contains "${RELEASE_EVIDENCE}" 'MAX_AGE_SECONDS = 14400'
assert_contains "${RELEASE_EVIDENCE}" 'select_latest_artifact'
assert_contains "${RELEASE_EVIDENCE}" 'release-candidate.py'
assert_contains "${RELEASE_EVIDENCE}" '--expected-staging-run-id'
assert_contains "${RELEASE_EVIDENCE}" '--expected-candidate-run-id'
assert_contains "${RELEASE_EVIDENCE}" 'GITHUB_EVENT_NAME'
assert_contains "${RELEASE_EVIDENCE}" 'merge_commit_sha'
assert_contains "${RELEASE_EVIDENCE}" 'fetch_commit_tree'
assert_contains "${RELEASE_EVIDENCE}" '!= tree_sha'
assert_contains "${RELEASE_EVIDENCE_SELFTEST}" 'ordinary-approved-pr-skipped-old-job'
assert_contains "${RELEASE_EVIDENCE_SELFTEST}" 'missing-artifact'
assert_contains "${RELEASE_EVIDENCE_SELFTEST}" 'stale-artifact'
assert_contains "${RELEASE_EVIDENCE_SELFTEST}" 'mismatched-staging-head'
assert_contains "${RELEASE_EVIDENCE_SELFTEST}" 'detect_main_reuse'

PYTHONDONTWRITEBYTECODE=1 python3 "${RELEASE_EVIDENCE_SELFTEST}"

python3 - "${CI_WORKFLOW}" "${FULL_CI_WORKFLOW}" "${STAGING_DEPLOYABILITY}" <<'PY'
import re
import sys
from pathlib import Path

for path_text in sys.argv[1:]:
    path = Path(path_text)
    workflow = path.read_text()
    detector = re.search(
        r"^  release-evidence:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:\n|\Z)",
        workflow,
        re.MULTILINE | re.DOTALL,
    )
    if not detector:
        raise SystemExit(f"{path} is missing release-evidence detector")
    detector_body = detector.group("body")
    for marker in (
        "    runs-on: homelab\n",
        "actions: read",
        "HOSTLET_RELEASE_EVIDENCE_MODE: main",
        "github.event.before",
        "reuse_heavy: ${{ steps.detect.outputs.reuse_heavy }}",
        "scripts/ci-release-evidence.py",
        "trusted release evidence evaluator is absent on the pre-push base",
        'echo "reuse_heavy=false" >> "${GITHUB_OUTPUT}"',
    ):
        if marker not in detector_body:
            raise SystemExit(f"{path} release-evidence detector missing {marker}")
    jobs_section = workflow.split("\njobs:\n", 1)[1]
    jobs = re.findall(r"^  ([a-zA-Z0-9_-]+):\n", jobs_section, re.MULTILINE)
    for name in jobs:
        if name in {"release-evidence", "self-hosted-api"}:
            continue
        body_match = re.search(
            rf"^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        )
        body = body_match.group("body") if body_match else ""
        if "runs-on: homelab-heavy\n" in body:
            if "needs: [release-evidence]\n" not in body:
                raise SystemExit(f"{path} heavy job {name} does not wait for release evidence")
            if "needs.release-evidence.outputs.reuse_heavy != 'true'" not in body:
                raise SystemExit(f"{path} heavy job {name} does not gate reuse")
PY

for workflow in "${CI_WORKFLOW}" "${FULL_CI_WORKFLOW}" "${STAGING_DEPLOYABILITY}" "${PR_WORKFLOW}"; do
  python3 - "${workflow}" <<'PY'
import re
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
for action in re.findall(r"^\s+uses:\s+([^\s]+)$", workflow, re.MULTILINE):
    if action.startswith("./"):
        continue
    if "@" not in action or not re.fullmatch(r"[0-9a-f]{40}", action.rsplit("@", 1)[1]):
        raise SystemExit(f"workflow action is not full-SHA pinned: {action}")
PY
done

echo "core workflow contracts passed"
