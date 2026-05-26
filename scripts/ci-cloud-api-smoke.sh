#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER="hostlet-ci-postgres-${GITHUB_RUN_ID:-local}-$$"
API_PID=""

cleanup() {
  if [ -n "${API_PID}" ] && kill -0 "${API_PID}" >/dev/null 2>&1; then
    kill "${API_PID}" >/dev/null 2>&1 || true
    wait "${API_PID}" >/dev/null 2>&1 || true
  fi
  docker rm -f "${CONTAINER}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --name "${CONTAINER}" \
  -e POSTGRES_USER=hostlet \
  -e POSTGRES_PASSWORD=ci-only-not-a-secret-postgres \
  -e POSTGRES_DB=hostlet \
  -p 127.0.0.1::5432 \
  postgres:16-alpine >/dev/null

for _ in $(seq 1 60); do
  if docker exec "${CONTAINER}" pg_isready -U hostlet -d hostlet >/dev/null 2>&1 \
    && docker exec "${CONTAINER}" psql -U hostlet -d hostlet -c "select 1" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
sleep 1

PORT="$(docker port "${CONTAINER}" 5432/tcp | sed 's/.*://')"
if [ -z "${PORT}" ]; then
  echo "could not discover mapped Postgres port" >&2
  exit 1
fi

export HOSTLET_MODE=cloud
export DATABASE_URL="postgres://hostlet:ci-only-not-a-secret-postgres@127.0.0.1:${PORT}/hostlet"
export BIND_ADDR=127.0.0.1:18080
export PUBLIC_API_URL=http://127.0.0.1:18080
export PUBLIC_WEB_URL=http://127.0.0.1:3000
export PUBLIC_WEBHOOK_URL=http://127.0.0.1:18080
export HOSTLET_ALLOWED_WEB_ORIGINS=http://127.0.0.1:3000
export HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false
export HOSTLET_SETUP_TOKEN=ci-only-not-a-secret-setup-token-01
export HOSTLET_ALLOWED_GITHUB_LOGINS=ci-user
export ENCRYPTION_KEY=YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=
export JOB_SIGNING_SECRET=ci-only-not-a-secret-job-signing-01
export SESSION_SECRET=ci-only-not-a-secret-session-secret-01
export LOCAL_AGENT_TOKEN=ci-only-not-a-secret-agent-token-01
export GITHUB_WEBHOOK_SECRET=ci-only-not-a-secret-webhook-secret-01
export HOSTLET_BASE_DOMAIN=hostlet.cloud
export STRIPE_SECRET_KEY=sk_test_ci_only_not_a_secret
export STRIPE_PUBLISHABLE_KEY=pk_test_ci_only_not_a_secret
export STRIPE_WEBHOOK_SECRET=whsec_ci_only_not_a_secret
export STRIPE_PRICE_STUDENT=price_ci_student
export STRIPE_PRICE_STARTER=price_ci_starter
export STRIPE_PRICE_PRO=price_ci_pro
export HOSTLET_UPDATE_CHECKS=false
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}"

cd "${ROOT}"
HOSTLET_DB_TEST_URL="${DATABASE_URL}" cargo test -p hostlet-api cloud_db -- --nocapture --test-threads=1
cargo run -p hostlet-api > /tmp/hostlet-cloud-api-smoke.log 2>&1 &
API_PID="$!"

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:18080/health >/dev/null 2>&1; then
    exit 0
  fi
  if ! kill -0 "${API_PID}" >/dev/null 2>&1; then
    cat /tmp/hostlet-cloud-api-smoke.log >&2
    exit 1
  fi
  sleep 1
done

cat /tmp/hostlet-cloud-api-smoke.log >&2
echo "Hostlet cloud API smoke did not become healthy" >&2
exit 1
