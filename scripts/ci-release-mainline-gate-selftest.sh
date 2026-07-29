#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/scripts/ci-release-mainline-gate.sh"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-release-mainline.XXXXXX")"
trap 'rm -rf "${TMP_ROOT}"' EXIT

ORIGIN="${TMP_ROOT}/origin.git"
WORK="${TMP_ROOT}/work"

git init --quiet --bare "${ORIGIN}"
git init --quiet --initial-branch=main "${WORK}"
git -C "${WORK}" config user.name "Hostlet CI"
git -C "${WORK}" config user.email "ci@hostlet.invalid"
git -C "${WORK}" remote add origin "${ORIGIN}"

git -C "${WORK}" commit --quiet --allow-empty -m "main ancestor"
main_ancestor="$(git -C "${WORK}" rev-parse HEAD)"
git -C "${WORK}" commit --quiet --allow-empty -m "main tip"
main_tip="$(git -C "${WORK}" rev-parse HEAD)"
git -C "${WORK}" push --quiet --set-upstream origin main

git -C "${WORK}" switch --quiet --create side "${main_ancestor}"
git -C "${WORK}" commit --quiet --allow-empty -m "side commit"
side_commit="$(git -C "${WORK}" rev-parse HEAD)"
git -C "${WORK}" switch --quiet main

run_case() {
  local expected="$1"
  local name="$2"
  local event_name="$3"
  local github_ref="$4"
  local github_sha="$5"
  local output
  local status

  set +e
  output="$(
    cd "${WORK}"
    GITHUB_EVENT_NAME="${event_name}" \
      GITHUB_REF="${github_ref}" \
      GITHUB_SHA="${github_sha}" \
      bash "${SCRIPT}" 2>&1
  )"
  status="$?"
  set -e

  if [ "${expected}" = "pass" ] && [ "${status}" -ne 0 ]; then
    echo "${name}: expected pass, got ${status}: ${output}" >&2
    exit 1
  fi
  if [ "${expected}" = "fail" ] && [ "${status}" -eq 0 ]; then
    echo "${name}: expected refusal, got pass" >&2
    exit 1
  fi
}

run_case pass main-tip push refs/tags/v1.2.3 "${main_tip}"
run_case pass main-ancestor push refs/tags/v1.2.2 "${main_ancestor}"
run_case fail arbitrary-side-tag push refs/tags/v9.9.9 "${side_commit}"
run_case fail prerelease-tag push refs/tags/v1.2.3-rc.1 "${main_tip}"
run_case fail build-metadata-tag push refs/tags/v1.2.3+build.1 "${main_tip}"
run_case fail partial-version-tag push refs/tags/v1.2 "${main_tip}"
run_case fail extra-suffix-tag push refs/tags/v1.2.3oops "${main_tip}"
run_case pass main-dry-run workflow_dispatch refs/heads/main "${main_tip}"
run_case fail branch-dry-run workflow_dispatch refs/heads/feature "${main_tip}"
run_case fail non-tag-push push refs/heads/main "${main_tip}"
run_case fail invalid-sha push refs/tags/v1.2.3 not-a-sha

echo "ci-release-mainline-gate self-test passed"
