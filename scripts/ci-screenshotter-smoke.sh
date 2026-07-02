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

SMOKE_URL="$(python3 - <<'PY'
from urllib.parse import quote

html = """<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <style>
      body {
        margin: 0;
        min-height: 720px;
        font: 32px Arial, sans-serif;
        color: #052e2b;
        background:
          radial-gradient(circle at 18% 20%, #34d399 0 14%, transparent 15%),
          radial-gradient(circle at 82% 16%, #60a5fa 0 12%, transparent 13%),
          linear-gradient(135deg, #f8fafc 0%, #e0f2fe 42%, #d1fae5 100%);
      }
      main {
        padding: 72px;
      }
      h1 {
        margin: 0 0 24px;
        max-width: 760px;
        font-size: 72px;
        line-height: 0.95;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 22px;
        margin-top: 48px;
      }
      .card {
        min-height: 210px;
        border: 1px solid rgba(15, 23, 42, 0.14);
        border-radius: 18px;
        background: rgba(255, 255, 255, 0.78);
        box-shadow: 0 18px 45px rgba(15, 23, 42, 0.12);
        padding: 24px;
      }
      .bar {
        height: 18px;
        border-radius: 999px;
        margin-top: 18px;
        background: linear-gradient(90deg, #059669, #2563eb, #f59e0b);
      }
    </style>
  </head>
  <body>
    <main>
      <h1>Hostlet screenshotter smoke</h1>
      <p>Styled capture probe with enough visual entropy to exercise the production byte floor.</p>
      <section class="grid">
        <div class="card">Styled page<div class="bar"></div></div>
        <div class="card">Decoded layout<div class="bar"></div></div>
        <div class="card">Readable proof<div class="bar"></div></div>
      </section>
    </main>
  </body>
</html>"""

print("data:text/html," + quote(html))
PY
)"

docker run --rm \
  -v "${TMP_DIR}:/out" \
  "${IMAGE}" \
  "${SMOKE_URL}" \
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

run_redirect_block_test() {
  local label="$1"
  local location="$2"
  local redirect_port=""
  for port in 18080 18082 18083 18084 18085; do
    docker rm -f "${REDIRECT_CONTAINER}" >/dev/null 2>&1 || true
    if ! docker run -d --rm --network host --name "${REDIRECT_CONTAINER}" \
      --entrypoint node \
      "${IMAGE}" \
      -e "require('http').createServer((_, res) => { res.writeHead(302, { Location: '${location}' }); res.end(); }).listen(${port}, '127.0.0.1');" \
      > "${TMP_DIR}/redirect-container" 2> "${TMP_DIR}/redirect-start.log"; then
      continue
    fi
    if docker run --rm --network host --entrypoint node \
      "${IMAGE}" \
      -e "require('http').get('http://127.0.0.1:${port}/', (res) => process.exit(res.statusCode === 302 ? 0 : 1)).on('error', () => process.exit(1));" \
      >/dev/null 2>&1; then
      redirect_port="${port}"
      break
    fi
  done

  if [ -z "${redirect_port}" ]; then
    echo "redirect server did not start on a Chromium-safe port"
    cat "${TMP_DIR}/redirect-start.log" 2>/dev/null || true
    exit 1
  fi

  if docker run --rm --network host \
    "${IMAGE}" \
    "http://127.0.0.1:${redirect_port}/" \
    /out/blocked.jpg > "${TMP_DIR}/ssrf-${label}.log" 2>&1; then
    echo "SSRF regression: screenshotter followed ${label} redirect"
    cat "${TMP_DIR}/ssrf-${label}.log"
    exit 1
  fi

  if ! grep -q "blocked request to" "${TMP_DIR}/ssrf-${label}.log"; then
    echo "SSRF regression: expected 'blocked request to' marker not found for ${label}"
    cat "${TMP_DIR}/ssrf-${label}.log"
    exit 1
  fi
}

# SSRF regression: a tenant app can 302-redirect the host-networked browser to a
# host-local origin. The target origins are allowed, but the redirect hop to a
# different loopback origin must be blocked. --network host is intentional here
# so the negative test exercises the real production posture.
run_redirect_block_test "loopback" "http://127.0.0.1:18081/"
run_redirect_block_test "mapped-ipv6" "http://[::ffff:7f00:1]:18081/"

echo "screenshotter SSRF regression passed"
