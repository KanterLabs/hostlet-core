#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB_PID=""
PORT="${HOSTLET_WEB_SMOKE_PORT:-13000}"

cleanup() {
  if [ -n "${WEB_PID}" ] && kill -0 "${WEB_PID}" >/dev/null 2>&1; then
    kill "${WEB_PID}" >/dev/null 2>&1 || true
    wait "${WEB_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

cd "${ROOT}"
if [ ! -d apps/web/.next ]; then
  pnpm --dir apps/web build
fi

NEXT_PUBLIC_API_URL="${NEXT_PUBLIC_API_URL:-http://127.0.0.1:18080}" \
NEXT_PUBLIC_WEBHOOK_URL="${NEXT_PUBLIC_WEBHOOK_URL:-http://127.0.0.1:18080}" \
pnpm --dir apps/web exec next start -H 127.0.0.1 -p "${PORT}" >/tmp/hostlet-web-route-smoke.log 2>&1 &
WEB_PID="$!"

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${PORT}/" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "${WEB_PID}" >/dev/null 2>&1; then
    cat /tmp/hostlet-web-route-smoke.log >&2
    exit 1
  fi
  sleep 1
done

# Routes that must render. Top-level pages plus dynamic detail routes
# (the trailing path segment is an arbitrary id that exercises [id] pages).
ROUTES=(
  # Public / top-level pages
  /
  /login
  /apps
  /apps/new
  /logs
  /settings
  # Dynamic detail routes ([id] segments)
  /apps/smoke-app
  /deployments/smoke-deployment
)
for path in "${ROUTES[@]}"; do
  curl -fsS "http://127.0.0.1:${PORT}${path}" >/dev/null
done

# Security headers must be present on every response (checked on /).
headers="$(curl -fsSI "http://127.0.0.1:${PORT}/")"
printf '%s\n' "${headers}" | grep -qi '^x-frame-options: DENY'
printf '%s\n' "${headers}" | grep -qi '^x-content-type-options: nosniff'
printf '%s\n' "${headers}" | grep -qi '^content-security-policy:'
