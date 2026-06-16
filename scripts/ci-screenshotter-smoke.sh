#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${HOSTLET_SCREENSHOTTER_TEST_IMAGE:-hostlet-screenshotter-ci}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-screenshotter-smoke.XXXXXX")"
REDIRECT_CONTAINER="hostlet-screenshotter-redirect-$$"
trap 'docker rm -f "${REDIRECT_CONTAINER}" >/dev/null 2>&1 || true; rm -rf "${TMP_DIR}"' EXIT

if [ "${HOSTLET_SCREENSHOTTER_SKIP_BUILD:-0}" != "1" ]; then
  "${ROOT}/scripts/ci-docker-retry.sh" docker build -f "${ROOT}/apps/screenshotter/Dockerfile" -t "${IMAGE}" "${ROOT}"
fi

# The screenshotter now runs as non-root (pwuser). Make the bind-mounted output
# directory world-accessible so the container user can create files inside it.
chmod a+rwx "${TMP_DIR}"

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

# SSRF regression: a tenant app can 302-redirect the host-networked browser to a
# host-local origin. The target origin (127.0.0.1:PORT) is allowed, but the
# redirect hop to a different loopback origin must be blocked. --network host is
# intentional here so the negative test exercises the real production posture.
REDIRECT_PORT=""
for port in 18080 18082 18083 18084 18085; do
  docker rm -f "${REDIRECT_CONTAINER}" >/dev/null 2>&1 || true
  if ! docker run -d --rm --network host --name "${REDIRECT_CONTAINER}" \
    --entrypoint node \
    "${IMAGE}" \
    -e "require('http').createServer((_, res) => { res.writeHead(302, { Location: 'http://127.0.0.1:18081/' }); res.end(); }).listen(${port}, '127.0.0.1');" \
    > "${TMP_DIR}/redirect-container" 2> "${TMP_DIR}/redirect-start.log"; then
    continue
  fi
  if docker run --rm --network host --entrypoint node \
    "${IMAGE}" \
    -e "require('http').get('http://127.0.0.1:${port}/', (res) => process.exit(res.statusCode === 302 ? 0 : 1)).on('error', () => process.exit(1));" \
    >/dev/null 2>&1; then
    REDIRECT_PORT="${port}"
    break
  fi
done

if [ -z "${REDIRECT_PORT}" ]; then
  echo "redirect server did not start on a Chromium-safe port"
  cat "${TMP_DIR}/redirect-start.log" 2>/dev/null || true
  exit 1
fi

if docker run --rm --network host \
  "${IMAGE}" \
  "http://127.0.0.1:${REDIRECT_PORT}/" \
  /out/blocked.jpg > "${TMP_DIR}/ssrf.log" 2>&1; then
  echo "SSRF regression: screenshotter followed redirect to a loopback origin"
  cat "${TMP_DIR}/ssrf.log"
  exit 1
fi

if ! grep -q "blocked request to" "${TMP_DIR}/ssrf.log"; then
  echo "SSRF regression: expected 'blocked request to' marker not found"
  cat "${TMP_DIR}/ssrf.log"
  exit 1
fi

echo "screenshotter SSRF regression passed"
