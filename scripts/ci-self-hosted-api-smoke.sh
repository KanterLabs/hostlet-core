#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/ci-self-hosted-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/ci-self-hosted-lib.sh"
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

# json_get, signed_cookie, expect_status, and the Postgres/env bootstrap helpers
# are shared with ci-self-hosted-deploy-e2e.sh; see ci-self-hosted-lib.sh.

# Header building blocks for state-changing requests, populated once ${origin}
# is known (below). ORIGIN_CSRF is the origin + CSRF guard pair every mutating
# request must carry; JSON_CT adds the JSON content-type for requests with a
# body. Expand into a curl invocation as "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}".
ORIGIN_CSRF=()
JSON_CT=(-H "content-type: application/json")

# ---------------------------------------------------------------------------
# Bring up Postgres and the self-hosted API under test.
# ---------------------------------------------------------------------------
start_postgres_container postgres:16-alpine
wait_postgres_ready
POSTGRES_PORT="$(discover_postgres_port)"

export_self_hosted_env "${POSTGRES_PORT}" "${API_PORT}"
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
ORIGIN_CSRF=(-H "origin: ${origin}" -H "x-hostlet-csrf: 1")

# ---------------------------------------------------------------------------
# First-run setup status: self-hosted mode, setup required.
# ---------------------------------------------------------------------------
status_payload="$(curl -fsS "${base}/api/setup/status")"
if [ "$(printf '%s' "${status_payload}" | json_get mode)" != "self_hosted" ]; then
  echo "setup status did not report self_hosted mode" >&2
  exit 1
fi
if [ "$(printf '%s' "${status_payload}" | json_get setupRequired)" != "true" ]; then
  echo "setup status did not require first-run setup" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Auth: setup requires the setup token + a strong password; logout/unlock cycle.
# ---------------------------------------------------------------------------
expect_status 401 -X POST "${base}/api/setup" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" --data '{"password":"ci-self-hosted-password"}'
expect_status 400 -X POST "${base}/api/setup" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" --data '{"password":"short"}'
expect_status 204 -c "${COOKIE_JAR}" -X POST "${base}/api/setup" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" --data '{"password":"ci-self-hosted-password"}'
expect_status 204 -b "${COOKIE_JAR}" -X POST "${base}/api/logout" "${ORIGIN_CSRF[@]}"
expect_status 401 -X POST "${base}/api/unlock" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" --data '{"password":"wrong-password"}'
expect_status 204 -c "${COOKIE_JAR}" -X POST "${base}/api/unlock" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" --data '{"password":"ci-self-hosted-password"}'

# ---------------------------------------------------------------------------
# Authenticate a CI user and confirm the session reports self-hosted mode.
# ---------------------------------------------------------------------------
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
# ---------------------------------------------------------------------------
# App CRUD: create preserves resource limits; PATCH allows CPU/RAM controls.
# ---------------------------------------------------------------------------
app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${base}/api/apps" --data "${create_payload}")"
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

expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X PATCH "${base}/api/apps/${app_id}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" --data '{"memory_limit_mb":1024,"cpu_limit":1,"health_path":"/ready","container_port":3000,"root_directory":"."}'
updated_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${base}/api/apps/${app_id}")"
if [ "$(printf '%s' "${updated_detail}" | json_get memoryLimitMb)" != "1024" ] || [ "$(printf '%s' "${updated_detail}" | json_get cpuLimit)" != "1" ]; then
  echo "self-hosted update did not allow CPU/RAM controls" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# App env vars: set, list, delete.
# ---------------------------------------------------------------------------
expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X PUT "${base}/api/apps/${app_id}/env/CI_EXTRA" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" --data '{"value":"extra-secret"}'
env_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${base}/api/apps/${app_id}/env")"
printf '%s' "${env_payload}" | grep -q 'CI_EXTRA'
expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X DELETE "${base}/api/apps/${app_id}/env/CI_EXTRA" "${ORIGIN_CSRF[@]}"

# ---------------------------------------------------------------------------
# Self-hosted mode does not expose the legacy OAuth route and disables agent
# install routes; clean up the app.
# ---------------------------------------------------------------------------
expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${base}/auth/github/oauth/start"
expect_status 410 -H "cookie: ${AUTH_COOKIE}" "${base}/api/servers/00000000-0000-0000-0000-000000000001/install"
expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X DELETE "${base}/api/apps/${app_id}" "${ORIGIN_CSRF[@]}"

echo "self-hosted API smoke passed"
