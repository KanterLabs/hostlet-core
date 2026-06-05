#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_WORKFLOW="${ROOT}/.github/workflows/staging.yml"
SELF_HOSTED_LIB="${ROOT}/scripts/ci-self-hosted-lib.sh"

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
assert_contains "${SELF_HOSTED_LIB}" 'export RUSTC="${RUSTC:-${rustc_path}}"'

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
