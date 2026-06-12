#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${HOSTLET_SCREENSHOTTER_TEST_IMAGE:-hostlet-screenshotter-ci}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-screenshotter-smoke.XXXXXX")"
REDIRECT_PID=""
trap 'kill "${REDIRECT_PID}" >/dev/null 2>&1 || true; rm -rf "${TMP_DIR}"' EXIT

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

# SSRF regression: a tenant app can 302-redirect the host-networked browser to a
# host-local origin. The target origin (127.0.0.1:PORT) is allowed, but the
# redirect hop to a different loopback origin must be blocked. --network host is
# intentional here so the negative test exercises the real production posture.
python3 - "${TMP_DIR}/redirect-port" <<'PY' &
import http.server
import socketserver
import sys
from pathlib import Path


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(302)
        self.send_header("Location", "http://127.0.0.1:9/")
        self.end_headers()

    def log_message(self, *args):
        pass


with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    Path(sys.argv[1]).write_text(str(httpd.server_address[1]))
    httpd.serve_forever()
PY
REDIRECT_PID=$!

REDIRECT_PORT=""
for _ in $(seq 1 50); do
  if [ -s "${TMP_DIR}/redirect-port" ]; then
    REDIRECT_PORT="$(cat "${TMP_DIR}/redirect-port")"
    break
  fi
  sleep 0.1
done

if [ -z "${REDIRECT_PORT}" ]; then
  echo "redirect server did not report a port"
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
