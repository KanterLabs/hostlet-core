#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER="hostlet-ci-cloud-e2e-postgres-${GITHUB_RUN_ID:-local}-$$"
API_PID=""
TMP_DIR="$(mktemp -d "/tmp/hostlet-cloud-e2e-${GITHUB_RUN_ID:-local}-$$.XXXXXX")"
API_PORT="${HOSTLET_CLOUD_E2E_PORT:-18083}"
API_LOG="${TMP_DIR}/api.log"
SESSION_SECRET="ci-only-not-a-secret-session-secret-01"
CLOUD_TOKEN="ci-cloud-session-token"
COOKIE_HEADER_FILE="${TMP_DIR}/cookie.txt"

cleanup() {
  if [ -n "${API_PID}" ] && kill -0 "${API_PID}" >/dev/null 2>&1; then
    kill "${API_PID}" >/dev/null 2>&1 || true
    wait "${API_PID}" >/dev/null 2>&1 || true
  fi
  docker rm -f "${CONTAINER}" >/dev/null 2>&1 || true
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

json_get() {
  node -e "let s=''; process.stdin.on('data', d => s += d); process.stdin.on('end', () => { const path = process.argv[1].split('.'); let v = JSON.parse(s); for (const key of path) v = v?.[key]; if (v === undefined || v === null) process.exit(2); process.stdout.write(String(v)); });" "$1"
}

expect_status() {
  local expected="$1"
  shift
  local actual
  actual="$(curl -sS -o "${TMP_DIR}/response.txt" -w "%{http_code}" "$@")"
  if [ "${actual}" != "${expected}" ]; then
    echo "Expected HTTP ${expected}, got ${actual}: $*" >&2
    cat "${TMP_DIR}/response.txt" >&2 || true
    exit 1
  fi
}

signed_cookie() {
  node -e '
    const crypto = require("crypto");
    const secret = process.argv[1];
    const value = process.argv[2];
    const payload = Buffer.from(value).toString("base64url");
    const expires = Math.floor(Date.now() / 1000) + 3600;
    const data = `v2.${payload}.${expires}`;
    const sig = "sha256=" + crypto.createHmac("sha256", secret).update(data).digest("hex");
    process.stdout.write(`${data}.${sig}`);
  ' "${SESSION_SECRET}" "$1"
}

token_hash() {
  node -e 'const crypto = require("crypto"); process.stdout.write(crypto.createHash("sha256").update(process.argv[1]).digest("base64"));' "$1"
}

docker run -d --name "${CONTAINER}" \
  -e POSTGRES_USER=hostlet \
  -e POSTGRES_PASSWORD=ci-only-not-a-secret-postgres \
  -e POSTGRES_DB=hostlet \
  -p 127.0.0.1::5432 \
  postgres:16-alpine >/dev/null

for _ in $(seq 1 60); do
  if docker exec "${CONTAINER}" pg_isready -U hostlet -d hostlet >/dev/null 2>&1 &&
    docker exec "${CONTAINER}" psql -U hostlet -d hostlet -c 'select 1' >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

PORT="$(docker port "${CONTAINER}" 5432/tcp | sed 's/.*://')"
if [ -z "${PORT}" ]; then
  echo "could not discover mapped Postgres port" >&2
  exit 1
fi

export HOSTLET_MODE=cloud
export DATABASE_URL="postgres://hostlet:ci-only-not-a-secret-postgres@127.0.0.1:${PORT}/hostlet"
export BIND_ADDR="127.0.0.1:${API_PORT}"
export PUBLIC_API_URL="http://127.0.0.1:${API_PORT}"
export PUBLIC_WEB_URL=http://127.0.0.1:3000
export PUBLIC_WEBHOOK_URL="http://127.0.0.1:${API_PORT}"
export HOSTLET_ALLOWED_WEB_ORIGINS=http://127.0.0.1:3000
export HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false
export HOSTLET_SETUP_TOKEN=ci-only-not-a-secret-setup-token-01
export HOSTLET_ALLOWED_GITHUB_LOGINS=ci-user
export ENCRYPTION_KEY=YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=
export JOB_SIGNING_SECRET=ci-only-not-a-secret-job-signing-01
export SESSION_SECRET
export LOCAL_AGENT_TOKEN=ci-only-not-a-secret-agent-token-01
export GITHUB_WEBHOOK_SECRET=ci-only-not-a-secret-webhook-secret-01
export HOSTLET_BASE_DOMAIN=hostlet.cloud
export GITHUB_APP_ID=1
export GITHUB_APP_SLUG=hostlet-ci
export GITHUB_APP_CLIENT_ID=ci-client-id
export GITHUB_APP_CLIENT_SECRET=ci-only-not-a-secret-github-app-client
export GITHUB_APP_PRIVATE_KEY_PEM=ci-only-not-a-secret-private-key
export GITHUB_APP_WEBHOOK_SECRET=ci-only-not-a-secret-github-app-webhook
export STRIPE_SECRET_KEY=sk_test_ci_only_not_a_secret
export STRIPE_PUBLISHABLE_KEY=pk_test_ci_only_not_a_secret
export STRIPE_WEBHOOK_SECRET=whsec_ci_only_not_a_secret
export STRIPE_PRICE_STUDENT=price_ci_student
export STRIPE_PRICE_STARTER=price_ci_starter
export STRIPE_PRICE_PRO=price_ci_pro
export HOSTLET_UPDATE_CHECKS=false
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}"

cd "${ROOT}"
cargo run -p hostlet-api >"${API_LOG}" 2>&1 &
API_PID="$!"

BASE_URL="http://127.0.0.1:${API_PORT}"
ORIGIN="http://127.0.0.1:3000"
for _ in $(seq 1 90); do
  if curl -fsS "${BASE_URL}/health" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "${API_PID}" >/dev/null 2>&1; then
    cat "${API_LOG}" >&2
    exit 1
  fi
  sleep 1
done

cloud_status="$(curl -fsS "${BASE_URL}/api/setup/status")"
if [ "$(printf '%s' "${cloud_status}" | json_get mode)" != "cloud" ]; then
  echo "setup status did not report cloud mode" >&2
  exit 1
fi
expect_status 403 -X POST "${BASE_URL}/api/setup" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"password":"ci-self-hosted-password"}'
expect_status 403 -X POST "${BASE_URL}/api/unlock" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"password":"ci-self-hosted-password"}'

USER_ID="00000000-0000-0000-0000-000000000101"
CLOUD_USER_ID="00000000-0000-0000-0000-000000000201"
CLOUD_TOKEN_HASH="$(token_hash "${CLOUD_TOKEN}")"
docker exec -i "${CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
INSERT INTO users (id, github_id, login) VALUES ('${USER_ID}', 9001, 'ci-user') ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login;
INSERT INTO cloud_users (id, github_id, login) VALUES ('${CLOUD_USER_ID}', 9001, 'ci-user') ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login, status='active';
INSERT INTO cloud_sessions (cloud_user_id, token_hash, expires_at) VALUES ('${CLOUD_USER_ID}', '${CLOUD_TOKEN_HASH}', now() + interval '1 hour') ON CONFLICT (token_hash) DO NOTHING;
SQL

SESSION_COOKIE="$(signed_cookie "${USER_ID}")"
printf 'hostlet_session=%s; hostlet_cloud_session=%s' "${SESSION_COOKIE}" "${CLOUD_TOKEN}" > "${COOKIE_HEADER_FILE}"
COOKIE_HEADER="$(cat "${COOKIE_HEADER_FILE}")"

create_payload='{
  "name":"cloud-e2e",
  "repo_full_name":"hostlet-ci/node-hello",
  "branch":"main",
  "server_id":null,
  "container_port":3000,
  "health_path":"/health",
  "domain":"",
  "runtime_kind":"single",
  "root_directory":".",
  "public_exposure":true,
  "auto_deploy":false,
  "deploy_after_create":false,
  "env":[]
}'
expect_status 402 -H "cookie: ${COOKIE_HEADER}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps" --data "${create_payload}"

docker exec -i "${CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
INSERT INTO cloud_github_installations (cloud_user_id, installation_id, account_login, account_type, permissions_json, repository_selection)
VALUES ('${CLOUD_USER_ID}', 12345, 'ci-user', 'User', '{}'::jsonb, 'selected')
ON CONFLICT (installation_id) DO UPDATE SET cloud_user_id=EXCLUDED.cloud_user_id, suspended_at=NULL;
INSERT INTO cloud_subscriptions (cloud_user_id, stripe_subscription_id, plan_code, status)
VALUES ('${CLOUD_USER_ID}', 'sub_ci_cloud_e2e', 'starter', 'active')
ON CONFLICT (stripe_subscription_id) DO UPDATE SET status=EXCLUDED.status;
SQL

expect_status 400 -H "cookie: ${COOKIE_HEADER}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps" --data "$(printf '%s' "${create_payload}" | node -e 'let s=""; process.stdin.on("data", d=>s+=d); process.stdin.on("end",()=>{const j=JSON.parse(s); j.memory_limit_mb=2048; process.stdout.write(JSON.stringify(j));})')"
expect_status 400 -H "cookie: ${COOKIE_HEADER}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps" --data "$(printf '%s' "${create_payload}" | node -e 'let s=""; process.stdin.on("data", d=>s+=d); process.stdin.on("end",()=>{const j=JSON.parse(s); j.runtime_kind="compose"; process.stdout.write(JSON.stringify(j));})')"

allowed_payload="$(printf '%s' "${create_payload}" | node -e 'let s=""; process.stdin.on("data", d=>s+=d); process.stdin.on("end",()=>{const j=JSON.parse(s); delete j.public_exposure; delete j.auto_deploy; process.stdout.write(JSON.stringify(j));})')"
app_payload="$(curl -fsS -H "cookie: ${COOKIE_HEADER}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps" --data "${allowed_payload}")"
app_id="$(printf '%s' "${app_payload}" | json_get id)"
app_detail="$(curl -fsS -H "cookie: ${COOKIE_HEADER}" "${BASE_URL}/api/apps/${app_id}")"
if [ "$(printf '%s' "${app_detail}" | json_get memoryLimitMb)" != "512" ] || [ "$(printf '%s' "${app_detail}" | json_get cpuLimit)" != "0.5" ]; then
  echo "cloud app did not receive fixed plan resources" >&2
  exit 1
fi
expect_status 400 -H "cookie: ${COOKIE_HEADER}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X PATCH "${BASE_URL}/api/apps/${app_id}" --data '{"memory_limit_mb":1024}'
expect_status 400 -H "cookie: ${COOKIE_HEADER}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X PATCH "${BASE_URL}/api/apps/${app_id}" --data '{"runtime_kind":"compose"}'

echo "cloud API E2E passed"
