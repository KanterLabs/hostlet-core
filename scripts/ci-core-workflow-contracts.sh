#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_WORKFLOW="${ROOT}/.github/workflows/staging.yml"

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
