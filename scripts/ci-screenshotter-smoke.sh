#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${HOSTLET_SCREENSHOTTER_TEST_IMAGE:-hostlet-screenshotter-ci}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-screenshotter-smoke.XXXXXX")"
SMOKE_CONTAINER="hostlet-screenshotter-smoke-$$"
REDIRECT_CONTAINER="hostlet-screenshotter-redirect-$$"
FIXTURE_CONTAINER="hostlet-screenshotter-fixture-$$"
trap 'docker rm -f "${SMOKE_CONTAINER}" "${REDIRECT_CONTAINER}" "${FIXTURE_CONTAINER}" >/dev/null 2>&1 || true; rm -rf "${TMP_DIR}"' EXIT

if [ "${HOSTLET_SCREENSHOTTER_SKIP_BUILD:-0}" != "1" ]; then
  "${ROOT}/scripts/ci-docker-retry.sh" docker build -f "${ROOT}/apps/screenshotter/Dockerfile" -t "${IMAGE}" "${ROOT}"
fi

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

# Keep the output in the container's writable layer and copy it out after the
# process exits. A bind mount here is unreliable on containerized CI runners:
# the Docker daemon may resolve the host path outside the runner container,
# where chmod from the job does not affect the mounted directory.
docker create --name "${SMOKE_CONTAINER}" \
  -e HOSTLET_BROWSER_SMOKE=1 \
  "${IMAGE}" \
  "${SMOKE_URL}" \
  /tmp/screenshot.webp >/dev/null
docker start --attach "${SMOKE_CONTAINER}"
docker cp "${SMOKE_CONTAINER}:/tmp/screenshot.webp" "${TMP_DIR}/screenshot.webp"
docker rm "${SMOKE_CONTAINER}" >/dev/null

python3 - "${TMP_DIR}/screenshot.webp" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = path.read_bytes()
if len(data) < 128 or not (data.startswith(b"RIFF") and data[8:12] == b"WEBP"):
    raise SystemExit("screenshotter did not produce a WebP")
PY

echo "screenshotter smoke passed"

FIXTURE_PORT=""
for port in 18090 18091 18092 18093 18094; do
  docker rm -f "${FIXTURE_CONTAINER}" >/dev/null 2>&1 || true
  if ! docker run -d --rm --network host --name "${FIXTURE_CONTAINER}" \
    --entrypoint node \
    "${IMAGE}" \
    -e "require('http').createServer((req, res) => { if (req.url === '/challenge') { res.writeHead(403, { 'Content-Type': 'text/html', 'cf-mitigated': 'challenge' }); res.end('<!doctype html><title>Checking your browser</title>'); } else if (req.url === '/error') { res.writeHead(503, { 'Content-Type': 'text/html' }); res.end('<!doctype html><title>Unavailable</title>'); } else if (req.url === '/sparse') { res.writeHead(200, { 'Content-Type': 'text/html' }); res.end('<!doctype html><style>html,body{margin:0;height:100%;background:#123456}</style>'); } else { res.writeHead(404); res.end(); } }).listen(${port}, '127.0.0.1');" \
    >"${TMP_DIR}/fixture-container" 2>"${TMP_DIR}/fixture-start.log"; then
    continue
  fi
  if docker run --rm --network host --entrypoint node \
    "${IMAGE}" \
    -e "require('http').get('http://127.0.0.1:${port}/sparse', (res) => process.exit(res.statusCode === 200 ? 0 : 1)).on('error', () => process.exit(1));" \
    >/dev/null 2>&1; then
    FIXTURE_PORT="${port}"
    break
  fi
done

if [ -z "${FIXTURE_PORT}" ]; then
  echo "screenshotter fixture server did not start on a Chromium-safe port"
  cat "${TMP_DIR}/fixture-start.log" 2>/dev/null || true
  exit 1
fi

if docker run --rm --network host "${IMAGE}" \
  "http://127.0.0.1:${FIXTURE_PORT}/challenge" /tmp/challenge.webp \
  >"${TMP_DIR}/challenge.log" 2>&1; then
  echo "screenshotter accepted a Cloudflare challenge response"
  cat "${TMP_DIR}/challenge.log"
  exit 1
fi
grep -q "cf-mitigated: challenge" "${TMP_DIR}/challenge.log"

if docker run --rm --network host "${IMAGE}" \
  "http://127.0.0.1:${FIXTURE_PORT}/error" /tmp/http-error.webp \
  >"${TMP_DIR}/http-error.log" 2>&1; then
  echo "screenshotter accepted an HTTP error response"
  cat "${TMP_DIR}/http-error.log"
  exit 1
fi
grep -q "navigation returned HTTP 503" "${TMP_DIR}/http-error.log"

if ! docker run --rm --network host \
  -e HOSTLET_SCREENSHOT_MIN_BYTES=100000 \
  "${IMAGE}" "http://127.0.0.1:${FIXTURE_PORT}/sparse" /tmp/sparse-manual.webp \
  >"${TMP_DIR}/sparse-manual.log" 2>&1; then
  echo "manual capture rejected a sparse but visually ready page"
  cat "${TMP_DIR}/sparse-manual.log"
  exit 1
fi
grep -q "retaining the manual capture" "${TMP_DIR}/sparse-manual.log"

if docker run --rm --network host \
  -e HOSTLET_BROWSER_SMOKE=1 \
  -e HOSTLET_SCREENSHOT_MIN_BYTES=100000 \
  "${IMAGE}" "http://127.0.0.1:${FIXTURE_PORT}/sparse" /tmp/sparse-smoke.webp \
  >"${TMP_DIR}/sparse-smoke.log" 2>&1; then
  echo "browser smoke accepted a sparse page below the byte floor"
  cat "${TMP_DIR}/sparse-smoke.log"
  exit 1
fi
grep -q "blank or near-blank" "${TMP_DIR}/sparse-smoke.log"

echo "screenshotter challenge and sparse-page regressions passed"

BLANK_URL="$(python3 - <<'PY'
from urllib.parse import quote
html = """<!doctype html><style>html,body{margin:0;height:100%;background:#173746;color:#fff;font:16px Arial}.hud{padding:20px}</style><div class=hud>CONNECTING...</div>"""
print("data:text/html," + quote(html))
PY
)"
if docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" "${BLANK_URL}" /tmp/blank.webp \
  >"${TMP_DIR}/blank.log" 2>&1; then
  echo "browser smoke accepted a blank loading shell"
  exit 1
fi
grep -q "blank or near-blank" "${TMP_DIR}/blank.log"

RUNTIME_ERROR_URL="$(python3 - <<'PY'
from urllib.parse import quote
html = """<!doctype html><style>body{background:linear-gradient(45deg,#123,#acf);min-height:720px}</style><script>setTimeout(()=>{throw new Error('startup exploded')},0)</script>"""
print("data:text/html," + quote(html))
PY
)"
if docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" "${RUNTIME_ERROR_URL}" /tmp/error.webp \
  >"${TMP_DIR}/runtime-error.log" 2>&1; then
  echo "browser smoke accepted an uncaught page error"
  exit 1
fi
grep -q "uncaught page error" "${TMP_DIR}/runtime-error.log"

CAUGHT_EVAL_URL="$(python3 - <<'PY'
from urllib.parse import quote
html = """<!doctype html><meta http-equiv="Content-Security-Policy" content="script-src 'nonce-hostlet'"><style>html,body{margin:0;background:#e0f2fe}canvas{width:100%;height:720px}</style><canvas width=1280 height=720></canvas><script nonce=hostlet>try{new Function('return 1')()}catch{}const c=document.querySelector('canvas'),x=c.getContext('2d');for(let y=0;y<720;y+=8){for(let i=0;i<1280;i+=8){x.fillStyle=`hsl(${(i*17+y*29)%360} 75% ${35+(i*y)%45}%)`;x.fillRect(i,y,8,8)}}</script>"""
print("data:text/html," + quote(html))
PY
)"
docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" "${CAUGHT_EVAL_URL}" /tmp/caught-eval.webp \
  >"${TMP_DIR}/caught-eval.log" 2>&1

if ! docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" \
  "data:application/json,%7B%22ok%22%3Atrue%7D" /tmp/non-html.webp \
  >"${TMP_DIR}/non-html.log" 2>&1; then
  echo "browser smoke did not skip a non-HTML endpoint"
  cat "${TMP_DIR}/non-html.log"
  exit 1
fi
grep -q "HOSTLET_BROWSER_SMOKE_SKIPPED_NON_HTML" "${TMP_DIR}/non-html.log"

echo "browser smoke readiness regressions passed"

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
