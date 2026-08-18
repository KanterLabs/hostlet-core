#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKFLOW="${ROOT}/.github/workflows/release-candidate.yml"

python3 - "${WORKFLOW}" <<'PY'
import re
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
match = re.search(
    r"^  seal-candidate:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:|\Z)",
    workflow,
    re.MULTILINE | re.DOTALL,
)
if not match:
    raise SystemExit("release candidate workflow is missing seal-candidate")

seal = match.group("body")
markers = (
    "test -s candidate-artifacts/hostlet-linux-x64",
    "sha256sum --check hostlet-linux-x64.sha256",
    "chmod +x candidate-artifacts/hostlet-linux-x64",
)
if any(marker not in seal for marker in markers):
    raise SystemExit("seal-candidate must verify transported CLI bytes and restore executable mode")
if [seal.index(marker) for marker in markers] != sorted(seal.index(marker) for marker in markers):
    raise SystemExit("seal-candidate must restore executable mode only after checksum verification")
if "test -x candidate-artifacts/hostlet-linux-x64" in seal:
    raise SystemExit("seal-candidate must not trust artifact transport to preserve executable mode")
PY

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT
printf 'hostlet-cli-fixture\n' > "${TMP_DIR}/hostlet-linux-x64"
chmod 0644 "${TMP_DIR}/hostlet-linux-x64"
(cd "${TMP_DIR}" && sha256sum hostlet-linux-x64 > hostlet-linux-x64.sha256)

test ! -x "${TMP_DIR}/hostlet-linux-x64"
test -s "${TMP_DIR}/hostlet-linux-x64"
(cd "${TMP_DIR}" && sha256sum --check hostlet-linux-x64.sha256)
chmod +x "${TMP_DIR}/hostlet-linux-x64"
test -x "${TMP_DIR}/hostlet-linux-x64"

echo "release artifact transport self-test passed"
