#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/ci-self-hosted-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/ci-self-hosted-lib.sh"
# shellcheck source=scripts/ci-metrics-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/ci-metrics-lib.sh"
RUN_ID="${GITHUB_RUN_ID:-local}-$$"
TMP_DIR="$(ci_tmp_dir hostlet-self-deploy "${RUN_ID}")"
POSTGRES_CONTAINER="hostlet-ci-self-deploy-postgres-${RUN_ID}"
API_PID=""
AGENT_PID=""
API_PORT="${HOSTLET_SELF_DEPLOY_API_PORT:-$(pick_local_port)}"
COOKIE_JAR="${TMP_DIR}/cookies.txt"
API_LOG="${TMP_DIR}/api.log"
AGENT_LOG="${TMP_DIR}/agent.log"
API_STARTUP_ATTEMPTS="${HOSTLET_SELF_HOSTED_STARTUP_ATTEMPTS:-300}"
AGENT_STARTUP_ATTEMPTS="${HOSTLET_SELF_HOSTED_AGENT_ATTEMPTS:-300}"
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
RAILPACK_BUILDKIT_CONTAINER="${HOSTLET_RAILPACK_BUILDKIT_CONTAINER:-hostlet-railpack-buildkit-${RUN_ID}}"

mark_failed() {
  local exit_code="$?"
  FAILED=1
  echo "self-hosted deploy E2E failed at line ${BASH_LINENO[0]} with exit code ${exit_code}; preserving ${TMP_DIR}" >&2
  exit "${exit_code}"
}
trap mark_failed ERR

cleanup() {
  local exit_code="$?"
  if [ "${exit_code}" -ne 0 ]; then
    FAILED=1
    echo "self-hosted deploy E2E failed with exit code ${exit_code}; preserving ${TMP_DIR}" >&2
    echo "--- agent log ---" >&2
    tail -200 "${AGENT_LOG}" >&2 || true
    echo "--- api log ---" >&2
    tail -200 "${API_LOG}" >&2 || true
  fi
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
    docker rm -f "${RAILPACK_BUILDKIT_CONTAINER}" >/dev/null 2>&1 || true
  fi
  for app in "${CREATED_APP_IDS[@]}"; do
    docker ps -aq --filter "name=hostlet-app-${app}" | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker volume rm "hostlet-app-data-${app}" >/dev/null 2>&1 || true
    docker images "hostlet/app-${app}" --format "{{.Repository}}:{{.Tag}}" | xargs -r docker image rm -f >/dev/null 2>&1 || true
    docker ps -aq --filter "label=com.docker.compose.project=hostlet-app-${app//-/}" | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker volume ls -q --filter "label=com.docker.compose.project=hostlet-app-${app//-/}" | xargs -r docker volume rm >/dev/null 2>&1 || true
    docker network ls -q --filter "label=com.docker.compose.project=hostlet-app-${app//-/}" | xargs -r docker network rm >/dev/null 2>&1 || true
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

wait_container_running() {
  local container="$1"
  local running=""
  for _ in $(seq 1 45); do
    running="$(docker inspect --format '{{.State.Running}}' "${container}" 2>/dev/null || true)"
    if [ "${running}" = "true" ]; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for ${container} to be running again" >&2
  docker inspect "${container}" >&2 || true
  tail -160 "${AGENT_LOG}" >&2 || true
  return 1
}

wait_published_health_ok() {
  local port="$1"
  local path="$2"
  local container="${3:-}"
  for _ in $(seq 1 45); do
    if curl -fsS "http://127.0.0.1:${port}${path}" 2>/dev/null | grep -q '^ok$'; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for published health on 127.0.0.1:${port}${path}" >&2
  if [ -n "${container}" ]; then
    docker inspect --format 'state={{.State.Status}} running={{.State.Running}} exit={{.State.ExitCode}} ports={{json .NetworkSettings.Ports}}' "${container}" >&2 || true
    docker logs --tail 80 "${container}" >&2 || true
  fi
  tail -160 "${AGENT_LOG}" >&2 || true
  return 1
}

wait_app_health_checked_after() {
  local target_app_id="$1"
  local previous_checked_at="$2"
  local payload="" status="" checked_at=""
  for _ in $(seq 1 45); do
    payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${target_app_id}/health")"
    status="$(printf '%s' "${payload}" | json_get status)"
    checked_at="$(printf '%s' "${payload}" | json_get lastCheckedAt || true)"
    if [ "${status}" = "healthy" ] && [ -n "${checked_at}" ] && [ "${checked_at}" != "${previous_checked_at}" ]; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for fresh healthy runtime status for app ${target_app_id}" >&2
  printf '%s\n' "${payload}" >&2
  tail -160 "${AGENT_LOG}" >&2 || true
  return 1
}

ensure_railpack() {
  if [ -n "${HOSTLET_RAILPACK_BIN:-}" ] && [ -x "${HOSTLET_RAILPACK_BIN}" ]; then
    return 0
  fi
  if [ -z "${HOSTLET_RAILPACK_BIN:-}" ] && command -v railpack >/dev/null 2>&1; then
    return 0
  fi
  export HOSTLET_RAILPACK_INSTALL_DIR="${HOSTLET_RAILPACK_INSTALL_DIR:-${TMP_DIR}/railpack-bin}"
  "${ROOT}/scripts/ci-install-railpack.sh"
  export HOSTLET_RAILPACK_BIN="${HOSTLET_RAILPACK_INSTALL_DIR}/railpack"
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

assert_runtime_metric() {
  local payload="$1"
  local path="$2"
  local mode="${3:-non_negative}"
  local value
  value="$(printf '%s' "${payload}" | json_get "${path}")"
  case "${mode}" in
    positive)
      printf '%s' "${value}" | grep -Eq '^[1-9][0-9]*$'
      ;;
    non_negative)
      printf '%s' "${value}" | grep -Eq '^[0-9]+$'
      ;;
    *)
      echo "unknown runtime metric assertion mode: ${mode}" >&2
      exit 1
      ;;
  esac
}

record_deployment_metric() {
  local fixture="$1"
  local scenario="$2"
  local detail="$3"
  if [ -z "${HOSTLET_RAILPACK_METRICS_FILE:-}" ]; then
    return 0
  fi
  python3 - "${fixture}" "${scenario}" "${detail}" <<'PY' | ci_metrics_upsert_object_by_fixture "${HOSTLET_RAILPACK_METRICS_FILE}"
import json
import sys

fixture = sys.argv[1]
scenario = sys.argv[2]
detail = json.loads(sys.argv[3])
metadata = ((detail.get("latestDeployment") or {}).get("runtimeMetadata") or {})


def int_value(key, default=None):
    value = metadata.get(key, default)
    if value is None:
        return None
    return int(value)


def include(payload, key, source_key):
    value = int_value(source_key)
    if value is not None:
        payload[key] = value


image_bytes = int_value("imageSizeBytes", 0)
build_duration_ms = int_value("buildDurationMs", 0)
boot_duration_ms = int_value("bootDurationMs", 0)
payload = {
    "fixture": fixture,
    "scenario": scenario,
    "imageBytes": image_bytes,
    "maxImageBytes": 500_000_000 if image_bytes > 0 else 1,
    "buildSeconds": build_duration_ms // 1000,
    "maxBuildSeconds": 300,
    "bootSeconds": boot_duration_ms // 1000,
    "maxBootSeconds": 120,
    "healthSeconds": boot_duration_ms // 1000,
    "maxHealthSeconds": 120,
}

for key in (
    "buildDurationMs",
    "containerStartDurationMs",
    "bootDurationMs",
    "gitSyncDurationMs",
    "buildPlanDurationMs",
    "healthCheckDurationMs",
    "routingDurationMs",
    "composeUpDurationMs",
):
    include(payload, key, key)

print(json.dumps(payload, sort_keys=True))
PY
}

# ---------------------------------------------------------------------------
# Bring up Postgres and the git fixture repo.
# ---------------------------------------------------------------------------
ensure_railpack
start_postgres_container postgres:16-alpine
wait_postgres_ready
POSTGRES_PORT="$(discover_postgres_port)"

make_fixture_repo "${APP_REPO_NAME}" "${ROOT}/scripts/fixtures/generated-apps/node"
make_fixture_repo "${COMPOSE_REPO_NAME}" "${ROOT}/scripts/fixtures/generated-apps/compose"
for fixture in "${RAILPACK_FIXTURES[@]}"; do
  IFS=: read -r fixture_name repo_name _health_path _expected <<<"${fixture}"
  make_fixture_repo "${repo_name}" "${ROOT}/scripts/fixtures/generated-apps/${fixture_name}"
done
if docker inspect "${RAILPACK_BUILDKIT_CONTAINER}" >/dev/null 2>&1; then
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
export HOSTLET_LOCAL_ROUTER=caddy
export HOSTLET_LOCAL_ROUTER_SNIPPETS_DIR="${TMP_DIR}/caddy"
export HOSTLET_LOCAL_ROUTER_RELOAD=true
export HOSTLET_RUNTIME_HEALTH_INTERVAL_SECONDS=2
export HOSTLET_RAILPACK_BUILDKIT_CONTAINER="${RAILPACK_BUILDKIT_CONTAINER}"
export GIT_CONFIG_GLOBAL
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMP_DIR}/target}"

cd "${ROOT}"
ci_build_binary hostlet-api hostlet-api
"$(ci_binary_path hostlet-api)" >"${API_LOG}" 2>&1 &
API_PID="$!"

BASE_URL="http://127.0.0.1:${API_PORT}"
ORIGIN="http://127.0.0.1:3000"
# Header building blocks for state-changing requests: ORIGIN_CSRF is the origin +
# CSRF guard pair, JSON_CT adds the JSON content-type for requests with a body.
ORIGIN_CSRF=(-H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1")
JSON_CT=(-H "content-type: application/json")
api_ready=0
for _ in $(seq 1 "${API_STARTUP_ATTEMPTS}"); do
  if curl -fsS "${BASE_URL}/health" >/dev/null 2>&1; then
    api_ready=1
    break
  fi
  if ! kill -0 "${API_PID}" >/dev/null 2>&1; then
    cat "${API_LOG}" >&2
    exit 1
  fi
  sleep 1
done
if [ "${api_ready}" != "1" ]; then
  echo "timed out waiting for self-hosted API after ${API_STARTUP_ATTEMPTS}s" >&2
  tail -200 "${API_LOG}" >&2 || true
  exit 1
fi

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

assert_container_readonly_rootfs() {
  local container="$1"
  local expected="$2"
  local actual
  actual="$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "${container}")"
  if [ "${actual}" != "${expected}" ]; then
    echo "expected ${container} ReadonlyRootfs=${expected}, got ${actual}" >&2
    exit 1
  fi
}

deploy_railpack_fixture() {
  local fixture_name="$1"
  local repo_name="$2"
  local health_path="$3"
  local expected="$4"
  local railpack_payload railpack_app_payload railpack_app_id railpack_deploy_payload railpack_deployment_id
  local railpack_detail railpack_published_port railpack_container railpack_logs railpack_delete_payload railpack_delete_job

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
  railpack_container="hostlet-app-${railpack_app_id}-${railpack_deployment_id}"
  curl -fsS "http://127.0.0.1:${railpack_published_port}${health_path}" >/dev/null
  curl -fsS "http://127.0.0.1:${railpack_published_port}/" | grep -q "${expected}"
  printf '%s' "${railpack_detail}" | json_get latestDeployment.runtimeMetadata.buildBackend | grep -q '^railpack$'
  printf '%s' "${railpack_detail}" | json_get latestDeployment.runtimeMetadata.packagingStrategy | grep -q '^generated$'
  printf '%s' "${railpack_detail}" | json_get latestDeployment.runtimeMetadata.readOnlyRootFilesystem | grep -q '^false$'
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.buildPlanDurationMs
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.buildDurationMs positive
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.imageSizeBytes positive
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.gitSyncDurationMs
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.containerStartDurationMs
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.healthCheckDurationMs
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.bootDurationMs
  assert_runtime_metric "${railpack_detail}" latestDeployment.runtimeMetadata.routingDurationMs

  railpack_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${railpack_deployment_id}/logs")"
  printf '%s' "${railpack_logs}" | grep -q 'Building generated runtime with Railpack'
  printf '%s' "${railpack_logs}" | grep -q 'Health check passed'
  docker ps --filter "name=hostlet-app-${railpack_app_id}" --format '{{.Ports}}' | grep -q '127.0.0.1'
  assert_container_readonly_rootfs "${railpack_container}" "false"
  record_deployment_metric "e2e-railpack-${fixture_name}" "selfHostedDeployE2e" "${railpack_detail}"

  railpack_delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${railpack_app_id}")"
  railpack_delete_job="$(printf '%s' "${railpack_delete_payload}" | json_get jobId)"
  wait_job_status "${railpack_delete_job}"
  expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${railpack_app_id}"
}

# deploy_managed_addons_app: a normal web app with a Hostlet-managed Postgres
# add-on. Create resolves it into a generated multi-service compose runtime; the
# agent builds the web image with Railpack, interpolates it + the generated
# secret into compose, and runs the stack. Asserts the per-service topology, that
# the backing Postgres is unpublished, that the web app actually reaches Postgres
# over the internal network via the injected DATABASE_URL, and that the secret
# never leaks into the deploy logs.
deploy_managed_addons_app() {
  local payload app_payload app_id detail published_port logs db_status project delete_payload delete_job
  payload="$(cat <<JSON
{
  "name":"ci-addons-postgres",
  "repo_full_name":"${APP_REPO_FULL}",
  "branch":"main",
  "server_id":null,
  "container_port":3000,
  "health_path":"/health",
  "domain":"",
  "runtime_kind":"single",
  "hostlet_config_path":"hostlet.yml",
  "runtime_config":{"compose":{"addOns":[{"key":"postgres"}],"backingMemoryLimitMb":256,"backingCpuLimit":0.25}},
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
  app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps" --data "${payload}")"
  app_id="$(printf '%s' "${app_payload}" | json_get id)"
  CREATED_APP_IDS+=("${app_id}")
  project="hostlet-app-${app_id//-/}"

  # Create switched the app to a compose runtime with a generated stack.
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  printf '%s' "${detail}" | json_get runtimeKind | grep -q '^compose$'

  deploy_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/deploy" --data '{}')"
  deployment_id="$(printf '%s' "${deploy_payload}" | json_get deploymentId)"
  wait_deployment_status "${deployment_id}"

  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  published_port="$(printf '%s' "${detail}" | json_get currentDeployment.publishedPort)"
  curl -fsS "http://127.0.0.1:${published_port}/health" | grep -q '^ok$'

  # Two services reported: web (routed) + postgres (backing).
  printf '%s' "${detail}" | python3 -c '
import sys, json
d = json.load(sys.stdin)
roles = {s["name"]: s["role"] for s in (d.get("services") or [])}
assert roles.get("web") == "web", roles
assert roles.get("postgres") == "backing", roles
'

  # The backing Postgres container has no host port; the web service publishes to loopback.
  if docker ps --filter "label=com.docker.compose.project=${project}" --filter "label=hostlet.role=backing" --format '{{.Ports}}' | grep -q '\->'; then
    echo "managed add-on backing service published a host port" >&2
    exit 1
  fi
  docker ps --filter "label=com.docker.compose.project=${project}" --filter "label=hostlet.role=web" --format '{{.Ports}}' | grep -q '127.0.0.1'

  # The backing Postgres carries the per-service caps from runtime_config
  # (256 MB = 268435456 bytes; 0.25 CPU = 250000000 NanoCpus).
  backing_container="$(docker ps --filter "label=com.docker.compose.project=${project}" --filter "label=hostlet.role=backing" --format '{{.Names}}' | head -1)"
  backing_mem="$(docker inspect --format '{{.HostConfig.Memory}}' "${backing_container}")"
  backing_cpus="$(docker inspect --format '{{.HostConfig.NanoCpus}}' "${backing_container}")"
  if [ "${backing_mem}" != "268435456" ] || [ "${backing_cpus}" != "250000000" ]; then
    echo "backing service caps not applied (mem=${backing_mem} nanocpus=${backing_cpus})" >&2
    exit 1
  fi

  # The web app reaches Postgres via the injected DATABASE_URL (give PG time to accept).
  db_status=""
  for _ in $(seq 1 30); do
    db_status="$(curl -fsS "http://127.0.0.1:${published_port}/db" || true)"
    if [ "${db_status}" = "db-ok" ]; then break; fi
    sleep 2
  done
  if [ "${db_status}" != "db-ok" ]; then
    echo "web app could not reach managed Postgres via DATABASE_URL (last: ${db_status})" >&2
    exit 1
  fi

  # The rendered connection secret never leaked into the deploy logs.
  logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
  if printf '%s' "${logs}" | grep -Eq 'postgres://postgres:[^@[:space:]]+@postgres'; then
    echo "managed add-on deploy logs leaked the rendered DATABASE_URL" >&2
    exit 1
  fi

  delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${app_id}")"
  delete_job="$(printf '%s' "${delete_payload}" | json_get jobId)"
  wait_job_status "${delete_job}"
  expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}"
  echo "managed add-ons (web + Postgres) E2E passed"
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
ci_build_binary hostlet-agent hostlet-agent
"$(ci_binary_path hostlet-agent)" >"${AGENT_LOG}" 2>&1 &
AGENT_PID="$!"

agent_ready=0
for _ in $(seq 1 "${AGENT_STARTUP_ATTEMPTS}"); do
  if curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/servers" | grep -q '"status":"online"'; then
    agent_ready=1
    break
  fi
  if ! kill -0 "${AGENT_PID}" >/dev/null 2>&1; then
    cat "${AGENT_LOG}" >&2
    exit 1
  fi
  sleep 1
done
if [ "${agent_ready}" != "1" ]; then
  echo "timed out waiting for self-hosted agent after ${AGENT_STARTUP_ATTEMPTS}s" >&2
  tail -200 "${AGENT_LOG}" >&2 || true
  exit 1
fi

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
  "runtime_config":{"dataMountPath":"/app/data"},
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
container_name="hostlet-app-${app_id}-${deployment_id}"
wait_published_health_ok "${published_port}" "/health" "${container_name}"
curl -fsS "http://127.0.0.1:${published_port}/" | grep -q 'hostlet-ci-v1-v1'

# The single-service managed volume honors the declared data path: it mounts at
# /app/data (runtime_config.dataMountPath) instead of the default /data, so apps
# that persist to a non-/data dir keep their data on the managed volume.
data_mount="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/app/data"}}{{.Type}} {{.Name}}{{end}}{{end}}' "${container_name}")"
if ! printf '%s' "${data_mount}" | grep -q "^volume hostlet-app-data-${app_id}$"; then
  echo "expected managed volume hostlet-app-data-${app_id} mounted at /app/data, got '${data_mount}'" >&2
  exit 1
fi
if docker inspect -f '{{range .Mounts}}{{println .Destination}}{{end}}' "${container_name}" | grep -qx "/data"; then
  echo "managed volume still mounted at /data despite dataMountPath=/app/data" >&2
  exit 1
fi

logs_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
printf '%s' "${logs_payload}" | grep -q 'Health check passed'
printf '%s' "${logs_payload}" | grep -q 'Building generated runtime with Railpack'
printf '%s' "${app_detail}" | json_get latestDeployment.runtimeMetadata.buildBackend | grep -q '^railpack$'
printf '%s' "${app_detail}" | json_get latestDeployment.runtimeMetadata.generatedDockerfile | grep -q '^false$'
printf '%s' "${app_detail}" | json_get latestDeployment.runtimeMetadata.readOnlyRootFilesystem | grep -q '^false$'
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.buildPlanDurationMs
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.buildDurationMs positive
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.imageSizeBytes positive
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.gitSyncDurationMs
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.containerStartDurationMs
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.healthCheckDurationMs
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.bootDurationMs
assert_runtime_metric "${app_detail}" latestDeployment.runtimeMetadata.routingDurationMs
if printf '%s' "${logs_payload}" | grep -q 'secret-value-for-redaction'; then
  echo "deployment logs exposed a raw secret" >&2
  exit 1
fi
docker ps --filter "name=hostlet-app-${app_id}" --format '{{.Ports}}' | grep -q '127.0.0.1'
assert_container_readonly_rootfs "${container_name}" "false"
record_deployment_metric "e2e-node-primary" "selfHostedDeployE2e" "${app_detail}"

# ---------------------------------------------------------------------------
# Runtime auto-start recovery: stop the current container on purpose and prove
# the recurring health loop starts it again after repeated failures.
# ---------------------------------------------------------------------------
wait_app_health_checked_after "${app_id}" ""
health_before_stop="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}/health")"
checked_before_stop="$(printf '%s' "${health_before_stop}" | json_get lastCheckedAt)"
started_before_stop="$(docker inspect --format '{{.State.StartedAt}}' "${container_name}")"
docker stop "${container_name}" >/dev/null
stopped_state="$(docker inspect --format '{{.State.Running}}' "${container_name}")"
if [ "${stopped_state}" != "false" ]; then
  echo "expected ${container_name} to be stopped, got Running=${stopped_state}" >&2
  exit 1
fi
if curl -fsS "http://127.0.0.1:${published_port}/health" >/dev/null 2>&1; then
  echo "expected stopped container health check to fail" >&2
  exit 1
fi
wait_container_running "${container_name}"
started_after_recovery="$(docker inspect --format '{{.State.StartedAt}}' "${container_name}")"
if [ "${started_after_recovery}" = "${started_before_stop}" ]; then
  echo "expected ${container_name} StartedAt to change after auto-start" >&2
  exit 1
fi
wait_app_health_checked_after "${app_id}" "${checked_before_stop}"
app_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
published_port="$(printf '%s' "${app_detail}" | json_get currentDeployment.publishedPort)"
wait_published_health_ok "${published_port}" "/health" "${container_name}"
recovery_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
printf '%s' "${recovery_logs}" | grep -q "Health checks failed and container is stopped; starting ${container_name}."
printf '%s' "${recovery_logs}" | grep -q "Auto-started container ${container_name}."

# ---------------------------------------------------------------------------
# Runtime route recovery: Docker can assign a new ephemeral loopback port after
# daemon or machine restart. Simulate stale Hostlet DB + route state and prove
# the health loop repairs both without a redeploy.
# ---------------------------------------------------------------------------
route_file="${HOSTLET_LOCAL_ROUTER_SNIPPETS_DIR}/app-${app_id}.caddy"
if [ ! -f "${route_file}" ]; then
  echo "expected local Caddy route snippet ${route_file}" >&2
  exit 1
fi
actual_port="$(docker inspect --format '{{(index (index .NetworkSettings.Ports "3000/tcp") 0).HostPort}}' "${container_name}")"
stale_port=9
docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
UPDATE deployments SET published_port=${stale_port} WHERE id='${deployment_id}';
SQL
sed -i "s/127\\.0\\.0\\.1:[0-9][0-9]*/127.0.0.1:${stale_port}/" "${route_file}"
health_before_port_repair="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}/health")"
checked_before_port_repair="$(printf '%s' "${health_before_port_repair}" | json_get lastCheckedAt || true)"
wait_app_health_checked_after "${app_id}" "${checked_before_port_repair}"
app_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
published_port="$(printf '%s' "${app_detail}" | json_get currentDeployment.publishedPort)"
if [ "${published_port}" != "${actual_port}" ]; then
  echo "expected health loop to repair published port to ${actual_port}, got ${published_port}" >&2
  exit 1
fi
grep -q "127.0.0.1:${actual_port}" "${route_file}"
wait_published_health_ok "${published_port}" "/health" "${container_name}"
port_repair_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
printf '%s' "${port_repair_logs}" | grep -q "Detected Docker-published port drift for ${container_name}; updating route from ${stale_port} to ${actual_port}."

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

# HOSTLET_E2E_SKIP_RAILPACK_FIXTURES=1 runs a focused subset (node + compose +
# add-ons) without the seven per-language Railpack fixture deploys. CI leaves it
# unset so the full matrix runs.
if [ "${HOSTLET_E2E_SKIP_RAILPACK_FIXTURES:-}" = "1" ]; then
  echo "skipping Railpack fixture deploys (HOSTLET_E2E_SKIP_RAILPACK_FIXTURES=1)"
else
  for fixture in "${RAILPACK_FIXTURES[@]}"; do
    IFS=: read -r fixture_name repo_name health_path expected <<<"${fixture}"
    deploy_railpack_fixture "${fixture_name}" "${repo_name}" "${health_path}" "${expected}"
  done
fi

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
assert_runtime_metric "${compose_detail}" latestDeployment.runtimeMetadata.composeUpDurationMs
assert_runtime_metric "${compose_detail}" latestDeployment.runtimeMetadata.gitSyncDurationMs
assert_runtime_metric "${compose_detail}" latestDeployment.runtimeMetadata.containerStartDurationMs
assert_runtime_metric "${compose_detail}" latestDeployment.runtimeMetadata.healthCheckDurationMs
assert_runtime_metric "${compose_detail}" latestDeployment.runtimeMetadata.bootDurationMs
assert_runtime_metric "${compose_detail}" latestDeployment.runtimeMetadata.routingDurationMs

compose_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${compose_deployment_id}/logs")"
printf '%s' "${compose_logs}" | grep -q 'Detected Hostlet Compose app'
printf '%s' "${compose_logs}" | grep -q 'Health check passed'
if printf '%s' "${compose_logs}" | grep -q 'compose-secret-value-for-redaction'; then
  echo "compose deployment logs exposed a raw secret" >&2
  exit 1
fi
docker ps --filter "label=com.docker.compose.project=hostlet-app-${compose_app_id//-/}" --format '{{.Ports}}' | grep -q '127.0.0.1'

# The fixture's web service persists to a relative host bind (./data:/app/data).
# The agent must auto-map that to a managed *named* volume — never a host bind —
# so assert the running web container mounts a volume (not a bind) at /app/data
# and that the app can actually read back what it wrote there.
compose_web_container="$(docker ps \
  --filter "label=com.docker.compose.project=hostlet-app-${compose_app_id//-/}" \
  --filter "label=com.docker.compose.service=web" --format '{{.ID}}' | head -1)"
compose_web_mount_type="$(docker inspect -f \
  '{{range .Mounts}}{{if eq .Destination "/app/data"}}{{.Type}}{{end}}{{end}}' \
  "${compose_web_container}")"
if [ "${compose_web_mount_type}" != "volume" ]; then
  echo "expected ./data to be auto-mapped to a managed named volume at /app/data, got mount type '${compose_web_mount_type}'" >&2
  exit 1
fi
curl -fsS "http://127.0.0.1:${compose_published_port}/data" | grep -q '^persisted-ok$'

# Storage quota: the agent measures managed-volume usage (docker system df -v) on
# its slow loop and the API serves it on the app JSON. The web service wrote a
# ~1 MB blob to /app/data, so poll for the first post-deploy sample, then assert
# the meter's used (>0) and the default 5 GB limit (5368709120 bytes).
compose_storage_used=0
for _ in $(seq 1 18); do
  compose_storage_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${compose_app_id}")"
  compose_storage_used="$(printf '%s' "${compose_storage_detail}" | json_get storageUsedBytes)"
  if [ "${compose_storage_used:-0}" -gt 0 ] 2>/dev/null; then
    break
  fi
  sleep 5
done
if ! [ "${compose_storage_used:-0}" -gt 0 ] 2>/dev/null; then
  echo "agent did not report managed-volume storage usage for the compose app (got '${compose_storage_used}')" >&2
  exit 1
fi
compose_storage_limit="$(printf '%s' "${compose_storage_detail}" | json_get storageLimitBytes)"
if [ "${compose_storage_limit}" != "5368709120" ]; then
  echo "expected the default 5 GB storage limit (5368709120), got '${compose_storage_limit}'" >&2
  exit 1
fi

record_deployment_metric "e2e-compose-fullstack" "selfHostedDeployE2e" "${compose_detail}"

compose_delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${compose_app_id}")"
compose_delete_job="$(printf '%s' "${compose_delete_payload}" | json_get jobId)"
wait_job_status "${compose_delete_job}"
expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${compose_app_id}"

# The agent's delete-job teardown must remove the compose project network
# (leaked networks exhaust Docker's address pool; see remove_compose_project_resources).
if docker network ls -q --filter "label=com.docker.compose.project=hostlet-app-${compose_app_id//-/}" | grep -q .; then
  echo "compose project network leaked after app delete" >&2
  exit 1
fi

# Managed add-ons (web app + Hostlet-managed Postgres) — the Phase 1b path.
deploy_managed_addons_app

echo "self-hosted deploy E2E passed"
