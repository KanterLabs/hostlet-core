#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${HOSTLET_SCREENSHOTTER_TEST_IMAGE:-hostlet-screenshotter-ci}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-screenshotter-smoke.XXXXXX")"
trap 'rm -rf "${TMP_DIR}"' EXIT

if [ "${HOSTLET_SCREENSHOTTER_SKIP_BUILD:-0}" != "1" ]; then
  docker build -f "${ROOT}/apps/screenshotter/Dockerfile" -t "${IMAGE}" "${ROOT}"
fi

docker run --rm \
  -v "${TMP_DIR}:/out" \
  "${IMAGE}" \
  'data:text/html,<main style="font:32px sans-serif">Hostlet screenshotter smoke</main>' \
  /out/screenshot.jpg

python3 - "${TMP_DIR}/screenshot.jpg" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = path.read_bytes()
if len(data) < 128 or not data.startswith(b"\xff\xd8\xff"):
    raise SystemExit("screenshotter did not produce a JPEG")
PY

echo "screenshotter smoke passed"
