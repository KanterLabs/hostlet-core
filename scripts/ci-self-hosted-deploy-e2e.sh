#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/ci-self-hosted-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/ci-self-hosted-lib.sh"
RUN_ID="${GITHUB_RUN_ID:-local}-$$"
TMP_DIR="$(mktemp -d "/tmp/hostlet-self-deploy-${RUN_ID}.XXXXXX")"
POSTGRES_CONTAINER="hostlet-ci-self-deploy-postgres-${RUN_ID}"
API_PID=""
AGENT_PID=""
API_PORT="${HOSTLET_SELF_DEPLOY_API_PORT:-18082}"
COOKIE_JAR="${TMP_DIR}/cookies.txt"
API_LOG="${TMP_DIR}/api.log"
AGENT_LOG="${TMP_DIR}/agent.log"
GIT_CONFIG_GLOBAL="${TMP_DIR}/gitconfig"
AUTH_COOKIE=""
APP_REPO_NAME="node-hello"
APP_REPO_FULL="hostlet-ci/${APP_REPO_NAME}"
COMPOSE_REPO_NAME="compose-fullstack"
COMPOSE_REPO_FULL="hostlet-ci/${COMPOSE_REPO_NAME}"
RAILPACK_FIXTURES=(
  "python:python-api:/health:hostlet-generated-python"
  "go:go-api:/health:hostlet-generated-go"
  "rust:rust-api:/health:hostlet-generated-rust"
  "static:static-site:/:hostlet-generated-static"
  "bun:bun-api:/health:hostlet-generated-bun"
  "yarn:yarn-api:/health:hostlet-generated-yarn"
  "next-pnpm:next-pnpm-site:/:hostlet-generated-next-pnpm"
)
CREATED_APP_IDS=()
RAILPACK_BUILDKIT_PREEXISTED=0
FAILED=0

mark_failed() {
  local exit_code="$?"
  FAILED=1
  echo "self-hosted deploy E2E failed at line ${BASH_LINENO[0]} with exit code ${exit_code}; preserving ${TMP_DIR}" >&2
  exit "${exit_code}"
}
trap mark_failed ERR

cleanup() {
  if [ -n "${AGENT_PID}" ] && kill -0 "${AGENT_PID}" >/dev/null 2>&1; then
    kill "${AGENT_PID}" >/dev/null 2>&1 || true
    wait "${AGENT_PID}" >/dev/null 2>&1 || true
  fi
  if [ -n "${API_PID}" ] && kill -0 "${API_PID}" >/dev/null 2>&1; then
    kill "${API_PID}" >/dev/null 2>&1 || true
    wait "${API_PID}" >/dev/null 2>&1 || true
  fi
  docker rm -f "${POSTGRES_CONTAINER}" >/dev/null 2>&1 || true
  if [ "${RAILPACK_BUILDKIT_PREEXISTED}" = "0" ]; then
    docker rm -f hostlet-railpack-buildkit >/dev/null 2>&1 || true
  fi
  for app in "${CREATED_APP_IDS[@]}"; do
    docker ps -aq --filter "name=hostlet-app-${app}" | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker volume rm "hostlet-app-data-${app}" >/dev/null 2>&1 || true
    docker images "hostlet/app-${app}" --format "{{.Repository}}:{{.Tag}}" | xargs -r docker image rm -f >/dev/null 2>&1 || true
    docker ps -aq --filter "label=com.docker.compose.project=hostlet-app-${app//-/}" | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker volume ls -q --filter "label=com.docker.compose.project=hostlet-app-${app//-/}" | xargs -r docker volume rm >/dev/null 2>&1 || true
  done
  if [ "${FAILED}" = "0" ]; then
    rm -rf "${TMP_DIR}"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# json_get, signed_cookie, expect_status, and the Postgres/env bootstrap helpers
# are shared with ci-self-hosted-api-smoke.sh; see ci-self-hosted-lib.sh.

# wait_deployment_status <deployment_id>: poll a deployment (up to ~180 * 2s)
# until it reaches a success/rolled_back terminal state; on `failed` dump the
# deployment logs plus the agent and API logs before failing.
wait_deployment_status() {
  local deployment_id="$1"
  local status=""
  for _ in $(seq 1 180); do
    payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}")"
    status="$(printf '%s' "${payload}" | json_get status)"
    case "${status}" in
      success|rolled_back)
        return 0
        ;;
      failed)
        echo "Deployment ${deployment_id} failed" >&2
        curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs" >&2 || true
        echo "--- agent log ---" >&2
        tail -200 "${AGENT_LOG}" >&2 || true
        echo "--- api log ---" >&2
        tail -200 "${API_LOG}" >&2 || true
        return 1
        ;;
    esac
    sleep 2
  done
  echo "Timed out waiting for deployment ${deployment_id}; last status=${status}" >&2
  tail -200 "${AGENT_LOG}" >&2 || true
  return 1
}

# wait_job_status <job_id>: poll an agent job (up to ~90 * 2s) until it reaches
# `success`; on `failed`/`canceled` print the job payload and agent log tail.
wait_job_status() {
  local job_id="$1"
  local status=""
  for _ in $(seq 1 90); do
    payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/agent-jobs/${job_id}")"
    status="$(printf '%s' "${payload}" | json_get status)"
    case "${status}" in
      success)
        return 0
        ;;
      failed|canceled)
        echo "Job ${job_id} ended with ${status}" >&2
        printf '%s\n' "${payload}" >&2
        tail -160 "${AGENT_LOG}" >&2 || true
        return 1
        ;;
    esac
    sleep 2
  done
  echo "Timed out waiting for job ${job_id}; last status=${status}" >&2
  return 1
}

make_fixture_repo() {
  local repo_name="$1"
  local fixture="$2"
  mkdir -p "${TMP_DIR}/git/${repo_name}"
  cd "${TMP_DIR}/git/${repo_name}"
  git init -b main >/dev/null
  git config user.email "ci@hostlet.local"
  git config user.name "Hostlet CI"
  cp -R "${fixture}/." .
  git add .
  git commit -m "initial app" >/dev/null
  git clone --bare . "${TMP_DIR}/git/${repo_name}.git" >/dev/null 2>&1
  cd "${ROOT}"
  cat > "${GIT_CONFIG_GLOBAL}" <<EOF
[url "file://${TMP_DIR}/git/"]
	insteadOf = https://github.com/hostlet-ci/
EOF
}

# ---------------------------------------------------------------------------
# Bring up Postgres and the git fixture repo.
# ---------------------------------------------------------------------------
start_postgres_container postgres:16-alpine
wait_postgres_ready
POSTGRES_PORT="$(discover_postgres_port)"

make_fixture_repo "${APP_REPO_NAME}" "${ROOT}/scripts/fixtures/generated-apps/node"
make_fixture_repo "${COMPOSE_REPO_NAME}" "${ROOT}/scripts/fixtures/generated-apps/compose"
for fixture in "${RAILPACK_FIXTURES[@]}"; do
  IFS=: read -r fixture_name repo_name _health_path _expected <<<"${fixture}"
  make_fixture_repo "${repo_name}" "${ROOT}/scripts/fixtures/generated-apps/${fixture_name}"
done
if docker inspect hostlet-railpack-buildkit >/dev/null 2>&1; then
  RAILPACK_BUILDKIT_PREEXISTED=1
fi

# ---------------------------------------------------------------------------
# Environment: shared self-hosted API config plus the agent-side config this
# E2E adds on top (agent token, job-signing secret, workdir, local mode).
# ---------------------------------------------------------------------------
export_self_hosted_env "${POSTGRES_PORT}" "${API_PORT}"
export HOSTLET_API_URL="http://127.0.0.1:${API_PORT}"
export HOSTLET_SERVER_ID="00000000-0000-0000-0000-000000000001"
export HOSTLET_AGENT_TOKEN="${LOCAL_AGENT_TOKEN}"
export HOSTLET_JOB_SIGNING_SECRET="${JOB_SIGNING_SECRET}"
export HOSTLET_WORKDIR="${TMP_DIR}/agent-work"
export HOSTLET_LOCAL_MODE=true
export HOSTLET_HEALTH_HOST=127.0.0.1
export GIT_CONFIG_GLOBAL
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}"

cd "${ROOT}"
cargo run -p hostlet-api >"${API_LOG}" 2>&1 &
API_PID="$!"

BASE_URL="http://127.0.0.1:${API_PORT}"
ORIGIN="http://127.0.0.1:3000"
# Header building blocks for state-changing requests: ORIGIN_CSRF is the origin +
# CSRF guard pair, JSON_CT adds the JSON content-type for requests with a body.
ORIGIN_CSRF=(-H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1")
JSON_CT=(-H "content-type: application/json")
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

# published_app_serves <expected-substring>: read the app's current published
# port and assert the running container serves a body containing the substring.
# Used to confirm which app version is live after deploy/redeploy/rollback.
published_app_serves() {
  local expected="$1"
  local detail port
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  port="$(printf '%s' "${detail}" | json_get currentDeployment.publishedPort)"
  curl -fsS "http://127.0.0.1:${port}/" | grep -q "${expected}"
}

deploy_railpack_fixture() {
  local fixture_name="$1"
  local repo_name="$2"
  local health_path="$3"
  local expected="$4"
  local railpack_payload railpack_app_payload railpack_app_id railpack_deploy_payload railpack_deployment_id
  local railpack_detail railpack_published_port railpack_logs railpack_delete_payload railpack_delete_job

  railpack_payload="$(cat <<JSON
{
  "name":"ci-railpack-${fixture_name}",
  "repo_full_name":"hostlet-ci/${repo_name}",
  "branch":"main",
  "server_id":null,
  "container_port":3000,
  "health_path":"${health_path}",
  "domain":"",
  "runtime_kind":"single",
  "hostlet_config_path":"hostlet.yml",
  "root_directory":".",
  "memory_limit_mb":512,
  "cpu_limit":0.5,
  "public_exposure":false,
  "auto_deploy":false,
  "deploy_after_create":false,
  "env":[]
}
JSON
)"
  railpack_app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps" --data "${railpack_payload}")"
  railpack_app_id="$(printf '%s' "${railpack_app_payload}" | json_get id)"
  CREATED_APP_IDS+=("${railpack_app_id}")

  railpack_deploy_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${railpack_app_id}/deploy" --data '{}')"
  railpack_deployment_id="$(printf '%s' "${railpack_deploy_payload}" | json_get deploymentId)"
  wait_deployment_status "${railpack_deployment_id}"

  railpack_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${railpack_app_id}")"
  railpack_published_port="$(printf '%s' "${railpack_detail}" | json_get currentDeployment.publishedPort)"
  curl -fsS "http://127.0.0.1:${railpack_published_port}${health_path}" >/dev/null
  curl -fsS "http://127.0.0.1:${railpack_published_port}/" | grep -q "${expected}"
  printf '%s' "${railpack_detail}" | json_get latestDeployment.runtimeMetadata.buildBackend | grep -q '^railpack$'
  printf '%s' "${railpack_detail}" | json_get latestDeployment.runtimeMetadata.packagingStrategy | grep -q '^generated$'

  railpack_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${railpack_deployment_id}/logs")"
  printf '%s' "${railpack_logs}" | grep -q 'Building generated runtime with Railpack'
  printf '%s' "${railpack_logs}" | grep -q 'Health check passed'
  docker ps --filter "name=hostlet-app-${railpack_app_id}" --format '{{.Ports}}' | grep -q '127.0.0.1'

  railpack_delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${railpack_app_id}")"
  railpack_delete_job="$(printf '%s' "${railpack_delete_payload}" | json_get jobId)"
  wait_job_status "${railpack_delete_job}"
  expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${railpack_app_id}"
}

# ---------------------------------------------------------------------------
# First-run setup + authenticate a CI user.
# ---------------------------------------------------------------------------
expect_status 204 -c "${COOKIE_JAR}" -X POST "${BASE_URL}/api/setup" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" --data '{"password":"ci-self-hosted-password"}'

user_id="00000000-0000-0000-0000-000000000101"
docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
INSERT INTO users (id, github_id, login) VALUES ('${user_id}', 9001, 'ci-user') ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login;
SQL
unlock_cookie="$(awk '$6 == "hostlet_unlock" { print $7 }' "${COOKIE_JAR}" | tail -1)"
AUTH_COOKIE="hostlet_unlock=${unlock_cookie}; hostlet_session=$(signed_cookie "${user_id}")"

# ---------------------------------------------------------------------------
# Start the agent and wait for it to register online.
# ---------------------------------------------------------------------------
cargo run -p hostlet-agent >"${AGENT_LOG}" 2>&1 &
AGENT_PID="$!"

for _ in $(seq 1 60); do
  if curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/servers" | grep -q '"status":"online"'; then
    break
  fi
  if ! kill -0 "${AGENT_PID}" >/dev/null 2>&1; then
    cat "${AGENT_LOG}" >&2
    exit 1
  fi
  sleep 1
done

create_payload="$(cat <<JSON
{
  "name":"ci-self-deploy",
  "repo_full_name":"${APP_REPO_FULL}",
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
  "env":[{"key":"APP_VERSION","value":"v1"},{"key":"CI_SECRET","value":"secret-value-for-redaction"}]
}
JSON
)"
# ---------------------------------------------------------------------------
# Create the app.
# ---------------------------------------------------------------------------
app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps" --data "${create_payload}")"
app_id="$(printf '%s' "${app_payload}" | json_get id)"
CREATED_APP_IDS+=("${app_id}")

# ---------------------------------------------------------------------------
# Deploy v1: container comes up healthy, serves v1, logs redact the secret, and
# the published port is bound to loopback only.
# ---------------------------------------------------------------------------
deploy_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/deploy" --data '{}')"
deployment_id="$(printf '%s' "${deploy_payload}" | json_get deploymentId)"
wait_deployment_status "${deployment_id}"

app_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
published_port="$(printf '%s' "${app_detail}" | json_get currentDeployment.publishedPort)"
curl -fsS "http://127.0.0.1:${published_port}/health" | grep -q '^ok$'
curl -fsS "http://127.0.0.1:${published_port}/" | grep -q 'hostlet-ci-v1-v1'

logs_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
printf '%s' "${logs_payload}" | grep -q 'Health check passed'
printf '%s' "${logs_payload}" | grep -q 'Building generated runtime with Railpack'
printf '%s' "${app_detail}" | json_get latestDeployment.runtimeMetadata.buildBackend | grep -q '^railpack$'
printf '%s' "${app_detail}" | json_get latestDeployment.runtimeMetadata.generatedDockerfile | grep -q '^false$'
if printf '%s' "${logs_payload}" | grep -q 'secret-value-for-redaction'; then
  echo "deployment logs exposed a raw secret" >&2
  exit 1
fi
docker ps --filter "name=hostlet-app-${app_id}" --format '{{.Ports}}' | grep -q '127.0.0.1'

# ---------------------------------------------------------------------------
# Redeploy v2: bumping APP_VERSION and redeploying serves v2 (data volume keeps
# the original v1 marker).
# ---------------------------------------------------------------------------
expect_status 204 -H "cookie: ${AUTH_COOKIE}" -X PUT "${BASE_URL}/api/apps/${app_id}/env/APP_VERSION" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" --data '{"value":"v2"}'
redeploy_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/deploy" --data '{}')"
redeploy_id="$(printf '%s' "${redeploy_payload}" | json_get deploymentId)"
wait_deployment_status "${redeploy_id}"
published_app_serves 'hostlet-ci-v2-v1'

# ---------------------------------------------------------------------------
# Rollback: reverts to the v1 image.
# ---------------------------------------------------------------------------
rollback_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/rollback" --data '{}')"
rollback_id="$(printf '%s' "${rollback_payload}" | json_get rollbackDeploymentId)"
wait_deployment_status "${rollback_id}"
published_app_serves 'hostlet-ci-v1-v1'

# ---------------------------------------------------------------------------
# Restart + on-demand health check: both run as agent jobs that succeed.
# ---------------------------------------------------------------------------
restart_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/restart" --data '{}')"
restart_job="$(printf '%s' "${restart_payload}" | json_get jobId)"
wait_job_status "${restart_job}"

health_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/health/check-now" --data '{}')"
health_job="$(printf '%s' "${health_payload}" | json_get jobId)"
wait_job_status "${health_job}"

# ---------------------------------------------------------------------------
# Delete: the teardown job completes and the app 404s afterward.
# ---------------------------------------------------------------------------
delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${app_id}")"
delete_job="$(printf '%s' "${delete_payload}" | json_get jobId)"
wait_job_status "${delete_job}"
expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}"

for fixture in "${RAILPACK_FIXTURES[@]}"; do
  IFS=: read -r fixture_name repo_name health_path expected <<<"${fixture}"
  deploy_railpack_fixture "${fixture_name}" "${repo_name}" "${health_path}" "${expected}"
done

compose_payload="$(cat <<JSON
{
  "name":"ci-compose-fullstack",
  "repo_full_name":"${COMPOSE_REPO_FULL}",
  "branch":"main",
  "server_id":null,
  "container_port":3000,
  "health_path":"/health",
  "domain":"",
  "runtime_kind":"compose",
  "hostlet_config_path":"hostlet.yml",
  "root_directory":".",
  "memory_limit_mb":512,
  "cpu_limit":0.5,
  "public_exposure":false,
  "auto_deploy":false,
  "deploy_after_create":false,
  "env":[{"key":"CI_SECRET","value":"compose-secret-value-for-redaction"}]
}
JSON
)"
compose_app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps" --data "${compose_payload}")"
compose_app_id="$(printf '%s' "${compose_app_payload}" | json_get id)"
CREATED_APP_IDS+=("${compose_app_id}")

compose_deploy_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${compose_app_id}/deploy" --data '{}')"
compose_deployment_id="$(printf '%s' "${compose_deploy_payload}" | json_get deploymentId)"
wait_deployment_status "${compose_deployment_id}"

compose_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${compose_app_id}")"
compose_published_port="$(printf '%s' "${compose_detail}" | json_get currentDeployment.publishedPort)"
curl -fsS "http://127.0.0.1:${compose_published_port}/health" | grep -q '^ok$'
curl -fsS "http://127.0.0.1:${compose_published_port}/" | grep -q 'hostlet-compose-fullstack'
printf '%s' "${compose_detail}" | json_get latestDeployment.runtimeMetadata.runtime | grep -q '^compose$'
printf '%s' "${compose_detail}" | json_get latestDeployment.runtimeMetadata.webService | grep -q '^web$'

compose_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${compose_deployment_id}/logs")"
printf '%s' "${compose_logs}" | grep -q 'Detected Hostlet Compose app'
printf '%s' "${compose_logs}" | grep -q 'Health check passed'
if printf '%s' "${compose_logs}" | grep -q 'compose-secret-value-for-redaction'; then
  echo "compose deployment logs exposed a raw secret" >&2
  exit 1
fi
docker ps --filter "label=com.docker.compose.project=hostlet-app-${compose_app_id//-/}" --format '{{.Ports}}' | grep -q '127.0.0.1'

compose_delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${compose_app_id}")"
compose_delete_job="$(printf '%s' "${compose_delete_payload}" | json_get jobId)"
wait_job_status "${compose_delete_job}"
expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${compose_app_id}"

echo "self-hosted deploy E2E passed"
