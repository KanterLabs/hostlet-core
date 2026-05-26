#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_ID="${GITHUB_RUN_ID:-local}-$$"
TMP_DIR="$(mktemp -d "/tmp/hostlet-self-api-${RUN_ID}.XXXXXX")"
POSTGRES_CONTAINER="hostlet-ci-self-api-postgres-${RUN_ID}"
API_PID=""
API_PORT="${HOSTLET_SELF_API_SMOKE_PORT:-18081}"
COOKIE_JAR="${TMP_DIR}/cookies.txt"
API_LOG="${TMP_DIR}/api.log"
AUTH_COOKIE=""

cleanup() {
  if [ -n "${API_PID}" ] && kill -0 "${API_PID}" >/dev/null 2>&1; then
    kill "${API_PID}" >/dev/null 2>&1 || true
    wait "${API_PID}" >/dev/null 2>&1 || true
  fi
  docker rm -f "${POSTGRES_CONTAINER}" >/dev/null 2>&1 || true
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

json_get() {
  node -e "let s=''; process.stdin.on('data', d => s += d); process.stdin.on('end', () => { const path = process.argv[1].split('.'); let v = JSON.parse(s); for (const key of path) v = v?.[key]; if (v === undefined || v === null) process.exit(2); process.stdout.write(String(v)); });" "$1"
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

docker run -d --name "${POSTGRES_CONTAINER}" \
  -e POSTGRES_USER=hostlet \
  -e POSTGRES_PASSWORD=ci-only-not-a-secret-postgres \
  -e POSTGRES_DB=hostlet \
  -p 127.0.0.1::5432 \
  postgres:16-alpine >/dev/null

for _ in $(seq 1 60); do
  if docker exec "${POSTGRES_CONTAINER}" pg_isready -U hostlet -d hostlet >/dev/null 2>&1 &&
    docker exec "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet -c 'select 1' >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

POSTGRES_PORT="$(docker port "${POSTGRES_CONTAINER}" 5432/tcp | sed 's/.*://')"
if [ -z "${POSTGRES_PORT}" ]; then
  echo "Could not discover mapped Postgres port" >&2
  exit 1
fi

export HOSTLET_MODE=self_hosted
export DATABASE_URL="postgres://hostlet:ci-only-not-a-secret-postgres@127.0.0.1:${POSTGRES_PORT}/hostlet"
export BIND_ADDR="127.0.0.1:${API_PORT}"
export PUBLIC_API_URL="http://127.0.0.1:${API_PORT}"
export PUBLIC_WEB_URL="http://127.0.0.1:3000"
export PUBLIC_WEBHOOK_URL="http://127.0.0.1:${API_PORT}"
export HOSTLET_ALLOWED_WEB_ORIGINS="http://127.0.0.1:3000"
export HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false
export HOSTLET_SETUP_TOKEN=ci-only-not-a-secret-setup-token-01
export HOSTLET_ALLOWED_GITHUB_LOGINS=ci-user
export ENCRYPTION_KEY=YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=
export JOB_SIGNING_SECRET=ci-only-not-a-secret-job-signing-01
export SESSION_SECRET=ci-only-not-a-secret-session-secret-01
export LOCAL_AGENT_TOKEN=ci-only-not-a-secret-agent-token-01
export GITHUB_WEBHOOK_SECRET=ci-only-not-a-secret-webhook-secret-01
export HOSTLET_UPDATE_CHECKS=false
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}"

cd "${ROOT}"
cargo run -p hostlet-api >"${API_LOG}" 2>&1 &
API_PID="$!"

for _ in $(seq 1 90); do
  if curl -fsS "http://127.0.0.1:${API_PORT}/health" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "${API_PID}" >/dev/null 2>&1; then
    cat "${API_LOG}" >&2
    exit 1
  fi
  sleep 1
done

base="http://127.0.0.1:${API_PORT}"
origin="http://127.0.0.1:3000"
status_payload="$(curl -fsS "${base}/api/setup/status")"
if [ "$(printf '%s' "${status_payload}" | json_get mode)" != "self_hosted" ]; then
  echo "setup status did not report self_hosted mode" >&2
  exit 1
fi
if [ "$(printf '%s' "${status_payload}" | json_get setupRequired)" != "true" ]; then
  echo "setup status did not require first-run setup" >&2
  exit 1
fi

expect_status 401 -X POST "${base}/api/setup" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"password":"ci-self-hosted-password"}'
expect_status 400 -X POST "${base}/api/setup" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" --data '{"password":"short"}'
expect_status 204 -c "${COOKIE_JAR}" -X POST "${base}/api/setup" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" --data '{"password":"ci-self-hosted-password"}'
expect_status 204 -b "${COOKIE_JAR}" -X POST "${base}/api/logout" -H "origin: ${origin}" -H "x-hostlet-csrf: 1"
expect_status 401 -X POST "${base}/api/unlock" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"password":"wrong-password"}'
expect_status 204 -c "${COOKIE_JAR}" -X POST "${base}/api/unlock" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"password":"ci-self-hosted-password"}'

user_id="00000000-0000-0000-0000-000000000101"
docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
INSERT INTO users (id, github_id, login) VALUES ('${user_id}', 9001, 'ci-user') ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login;
SQL
unlock_cookie="$(awk '$6 == "hostlet_unlock" { print $7 }' "${COOKIE_JAR}" | tail -1)"
AUTH_COOKIE="hostlet_unlock=${unlock_cookie}; hostlet_session=$(signed_cookie "${user_id}")"

session_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${base}/api/session")"
if [ "$(printf '%s' "${session_payload}" | json_get mode)" != "self_hosted" ]; then
  echo "session did not report self_hosted mode" >&2
  exit 1
fi

create_payload='{
  "name":"ci-self-api",
  "repo_full_name":"hostlet-ci/node-hello",
  "branch":"main",
  "server_id":null,
  "container_port":3000,
  "health_path":"/health",
  "domain":"",
  "runtime_kind":"single",
  "hostlet_config_path":"hostlet.yml",
  "root_directory":".",
  "memory_limit_mb":512,
  "cpu_limit":0.5,
  "public_exposure":false,
  "auto_deploy":false,
  "deploy_after_create":false,
  "env":[{"key":"CI_SECRET","value":"self-api-secret"}]
}'
app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${base}/api/apps" --data "${create_payload}")"
app_id="$(printf '%s' "${app_payload}" | json_get id)"

app_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${base}/api/apps/${app_id}")"
if [ "$(printf '%s' "${app_detail}" | json_get memoryLimitMb)" != "512" ]; then
  echo "self-hosted create did not preserve memory limit" >&2
  exit 1
fi
if [ "$(printf '%s' "${app_detail}" | json_get cpuLimit)" != "0.5" ]; then
  echo "self-hosted create did not preserve CPU limit" >&2
  exit 1
fi

expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X PATCH "${base}/api/apps/${app_id}" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"memory_limit_mb":1024,"cpu_limit":1,"health_path":"/ready","container_port":3000,"root_directory":"."}'
updated_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${base}/api/apps/${app_id}")"
if [ "$(printf '%s' "${updated_detail}" | json_get memoryLimitMb)" != "1024" ] || [ "$(printf '%s' "${updated_detail}" | json_get cpuLimit)" != "1" ]; then
  echo "self-hosted update did not allow CPU/RAM controls" >&2
  exit 1
fi

expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X PUT "${base}/api/apps/${app_id}/env/CI_EXTRA" -H "origin: ${origin}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' --data '{"value":"extra-secret"}'
env_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${base}/api/apps/${app_id}/env")"
printf '%s' "${env_payload}" | grep -q 'CI_EXTRA'
expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X DELETE "${base}/api/apps/${app_id}/env/CI_EXTRA" -H "origin: ${origin}" -H "x-hostlet-csrf: 1"

expect_status 403 -H "cookie: ${AUTH_COOKIE}" "${base}/auth/github/oauth/start"
expect_status 410 -H "cookie: ${AUTH_COOKIE}" "${base}/api/servers/00000000-0000-0000-0000-000000000001/install"
expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X DELETE "${base}/api/apps/${app_id}" -H "origin: ${origin}" -H "x-hostlet-csrf: 1"

echo "self-hosted API smoke passed"
