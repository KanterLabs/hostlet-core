#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
CREATED_APP_ID=""

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
  if [ -n "${CREATED_APP_ID}" ]; then
    docker ps -aq --filter "name=hostlet-app-${CREATED_APP_ID}" | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker volume rm "hostlet-app-data-${CREATED_APP_ID}" >/dev/null 2>&1 || true
  fi
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
  mkdir -p "${TMP_DIR}/git/${APP_REPO_NAME}"
  cd "${TMP_DIR}/git/${APP_REPO_NAME}"
  git init -b main >/dev/null
  git config user.email "ci@hostlet.local"
  git config user.name "Hostlet CI"
  cat > server.js <<'EOF'
const http = require("http");
const port = Number(process.env.PORT || 3000);
const version = process.env.APP_VERSION || "v1";
http.createServer((req, res) => {
  if (req.url === "/health") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("ok");
    return;
  }
  res.writeHead(200, { "content-type": "text/plain" });
  res.end(`hostlet-ci-${version}`);
}).listen(port, "0.0.0.0");
EOF
  cat > Dockerfile <<'EOF'
FROM node:22-alpine
WORKDIR /app
COPY server.js .
ENV PORT=3000
CMD ["node", "server.js"]
EOF
  git add .
  git commit -m "initial app" >/dev/null
  git clone --bare . "${TMP_DIR}/git/${APP_REPO_NAME}.git" >/dev/null 2>&1
  cd "${ROOT}"
  cat > "${GIT_CONFIG_GLOBAL}" <<EOF
[url "file://${TMP_DIR}/git/"]
	insteadOf = https://github.com/hostlet-ci/
EOF
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

make_fixture_repo

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

expect_status 204 -c "${COOKIE_JAR}" -X POST "${BASE_URL}/api/setup" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" --data '{"password":"ci-self-hosted-password"}'

user_id="00000000-0000-0000-0000-000000000101"
docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
INSERT INTO users (id, github_id, login) VALUES ('${user_id}', 9001, 'ci-user') ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login;
SQL
unlock_cookie="$(awk '$6 == "hostlet_unlock" { print $7 }' "${COOKIE_JAR}" | tail -1)"
AUTH_COOKIE="hostlet_unlock=${unlock_cookie}; hostlet_session=$(signed_cookie "${user_id}")"

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
app_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps" --data "${create_payload}")"
app_id="$(printf '%s' "${app_payload}" | json_get id)"
CREATED_APP_ID="${app_id}"

deploy_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -X POST "${BASE_URL}/api/apps/${app_id}/deploy" --data '{}')"
deployment_id="$(printf '%s' "${deploy_payload}" | json_get deploymentId)"
wait_deployment_status "${deployment_id}"

app_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
published_port="$(printf '%s' "${app_detail}" | json_get currentDeployment.publishedPort)"
curl -fsS "http://127.0.0.1:${published_port}/health" | grep -q '^ok$'
curl -fsS "http://127.0.0.1:${published_port}/" | grep -q 'hostlet-ci-v1'

logs_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
printf '%s' "${logs_payload}" | grep -q 'Health check passed'
if printf '%s' "${logs_payload}" | grep -q 'secret-value-for-redaction'; then
  echo "deployment logs exposed a raw secret" >&2
  exit 1
fi
docker ps --filter "name=hostlet-app-${CREATED_APP_ID}" --format '{{.Ports}}' | grep -q '127.0.0.1'

restart_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps/${app_id}/restart" --data '{}')"
restart_job="$(printf '%s' "${restart_payload}" | json_get jobId)"
wait_job_status "${restart_job}"

health_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X POST "${BASE_URL}/api/apps/${app_id}/health/check-now" --data '{}')"
health_job="$(printf '%s' "${health_payload}" | json_get jobId)"
wait_job_status "${health_job}"

delete_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" -H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1" -H 'content-type: application/json' -X DELETE "${BASE_URL}/api/apps/${app_id}")"
delete_job="$(printf '%s' "${delete_payload}" | json_get jobId)"
wait_job_status "${delete_job}"
expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}"

echo "self-hosted deploy E2E passed"
