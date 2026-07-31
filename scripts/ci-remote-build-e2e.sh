#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/ci-self-hosted-lib.sh
source "${ROOT}/scripts/ci-self-hosted-lib.sh"

RUN_ID="${GITHUB_RUN_ID:-local}-$$"
TMP_DIR="$(ci_tmp_dir hostlet-remote-build "${RUN_ID}")"
POSTGRES_CONTAINER="hostlet-ci-remote-postgres-${RUN_ID}"
REGISTRY_CONTAINER="hostlet-ci-remote-registry-${RUN_ID}"
PROXY_CONTAINER="hostlet-ci-remote-registry-tls-${RUN_ID}"
RUNNER_DIND="hostlet-ci-remote-runner-dind-${RUN_ID}"
BUILDER_DIND="hostlet-ci-remote-builder-dind-${RUN_ID}"
RUNNER_AGENT_CONTAINER="hostlet-ci-remote-runner-agent-${RUN_ID}"
BUILDER_AGENT_CONTAINER="hostlet-ci-remote-builder-agent-${RUN_ID}"
NETWORK="hostlet-ci-remote-${RUN_ID}"
API_PORT="$(pick_local_port)"
API_LOG="${TMP_DIR}/api.log"
RUNNER_LOG="${TMP_DIR}/runner.log"
BUILDER_LOG="${TMP_DIR}/builder.log"
COOKIE_JAR="${TMP_DIR}/cookies.txt"
GIT_CONFIG_GLOBAL="${TMP_DIR}/gitconfig"
API_PID=""
RUNNER_PID=""
BUILDER_PID=""
AUTH_COOKIE=""
POOL_ID=""
REGISTRY_HOST=""
REGISTRY_URL=""
REGISTRY_PUSH_USERNAME=hostlet-builder
REGISTRY_PUSH_PASSWORD=ci-builder-password
REGISTRY_PULL_USERNAME=hostlet-runner
REGISTRY_PULL_PASSWORD=ci-runner-password
declare -A FIXTURE_SHAS=()

cleanup() {
  local exit_code="$?"
  for pid in "${BUILDER_PID}" "${RUNNER_PID}" "${API_PID}"; do
    if [ -n "${pid}" ] && kill -0 "${pid}" >/dev/null 2>&1; then
      kill "${pid}" >/dev/null 2>&1 || true
      wait "${pid}" >/dev/null 2>&1 || true
    fi
  done
  docker exec "${RUNNER_AGENT_CONTAINER}" chown -R "$(id -u):$(id -g)" "${TMP_DIR}" >/dev/null 2>&1 || true
  docker exec "${BUILDER_AGENT_CONTAINER}" chown -R "$(id -u):$(id -g)" "${TMP_DIR}" >/dev/null 2>&1 || true
  if [ "${exit_code}" -ne 0 ]; then
    echo "remote build E2E failed; preserving ${TMP_DIR}" >&2
    docker logs --tail 200 "${BUILDER_AGENT_CONTAINER}" >&2 || true
    docker logs --tail 200 "${RUNNER_AGENT_CONTAINER}" >&2 || true
    tail -200 "${API_LOG}" >&2 || true
  fi
  docker rm -f "${BUILDER_AGENT_CONTAINER}" "${RUNNER_AGENT_CONTAINER}" \
    "${BUILDER_DIND}" "${RUNNER_DIND}" "${PROXY_CONTAINER}" \
    "${REGISTRY_CONTAINER}" "${POSTGRES_CONTAINER}" >/dev/null 2>&1 || true
  docker network rm "${NETWORK}" >/dev/null 2>&1 || true
  if [ "${exit_code}" -eq 0 ]; then
    rm -rf "${TMP_DIR}"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ensure_railpack() {
  if [ -n "${HOSTLET_RAILPACK_BIN:-}" ] && [ -x "${HOSTLET_RAILPACK_BIN}" ]; then
    return
  fi
  if [ -z "${HOSTLET_RAILPACK_BIN:-}" ] && command -v railpack >/dev/null 2>&1; then
    return
  fi
  export HOSTLET_RAILPACK_INSTALL_DIR="${TMP_DIR}/railpack-bin"
  "${ROOT}/scripts/ci-install-railpack.sh"
  export HOSTLET_RAILPACK_BIN="${HOSTLET_RAILPACK_INSTALL_DIR}/railpack"
}

make_fixture_repo() {
  local name="$1"
  local source="$2"
  mkdir -p "${TMP_DIR}/git/${name}"
  git -C "${TMP_DIR}/git/${name}" init -b main >/dev/null
  git -C "${TMP_DIR}/git/${name}" config user.email ci@hostlet.local
  git -C "${TMP_DIR}/git/${name}" config user.name "Hostlet CI"
  cp -R "${source}/." "${TMP_DIR}/git/${name}/"
  git -C "${TMP_DIR}/git/${name}" add .
  git -C "${TMP_DIR}/git/${name}" commit -m "initial app" >/dev/null
  FIXTURE_SHAS["${name}"]="$(git -C "${TMP_DIR}/git/${name}" rev-parse HEAD)"
  git clone --bare "${TMP_DIR}/git/${name}" "${TMP_DIR}/git/${name}.git" >/dev/null 2>&1
}

wait_api() {
  for _ in $(seq 1 180); do
    curl -fsS "${BASE_URL}/health" >/dev/null 2>&1 && return
    kill -0 "${API_PID}" >/dev/null 2>&1 || { cat "${API_LOG}" >&2; return 1; }
    sleep 1
  done
  return 1
}

wait_deployment() {
  local id="$1"
  local payload status
  for _ in $(seq 1 240); do
    payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${id}")"
    status="$(printf '%s' "${payload}" | json_get status)"
    case "${status}" in
      success|rolled_back) return ;;
      failed|canceled)
        echo "remote deployment ${id} ended ${status}" >&2
        curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${id}/logs" >&2 || true
        return 1
        ;;
    esac
    sleep 2
  done
  echo "remote deployment ${id} timed out in ${status}" >&2
  return 1
}

wait_agent() {
  local endpoint="$1"
  local id="$2"
  for _ in $(seq 1 120); do
    if curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}${endpoint}" | \
      python3 -c 'import json,sys; rows=json.load(sys.stdin); target=sys.argv[1]; raise SystemExit(0 if any(str(row.get("id")) == target and row.get("status") == "online" for row in rows) else 1)' "${id}"; then
      return
    fi
    sleep 1
  done
  return 1
}

configure_app_build_pool() {
  local app_id="$1"
  expect_status 204 -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" \
    -X PUT "${BASE_URL}/api/apps/${app_id}/build-pool" \
    --data "{\"buildPoolId\":\"${POOL_ID}\"}"
}

create_and_deploy() {
  local name="$1"
  local repo="$2"
  local runtime="$3"
  local config="$4"
  local port="${5:-3000}"
  local health="${6:-/health}"
  local create create_response create_status app_id deploy deployment_id detail published
  echo "remote E2E: creating ${name}" >&2
  create_response="$(curl -sS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" \
    -X POST "${BASE_URL}/api/apps" --data "$(cat <<JSON
{"name":"${name}","repo_full_name":"hostlet-ci/${repo}","branch":"main","server_id":null,"container_port":${port},"health_path":"${health}","domain":"${name}.localhost","runtime_kind":"${runtime}","hostlet_config_path":"hostlet.yml","root_directory":".","runtime_config":${config},"memory_limit_mb":512,"cpu_limit":0.5,"public_exposure":false,"auto_deploy":false,"deploy_after_create":false,"env":[{"key":"REMOTE_E2E_SECRET","value":"must-not-cross-artifact-boundary"}]}
JSON
)" -w $'\n%{http_code}')"
  create_status="${create_response##*$'\n'}"
  create="${create_response%$'\n'*}"
  if [[ ! "${create_status}" =~ ^2 ]]; then
    echo "create ${name} failed (${create_status}): ${create}" >&2
    return 1
  fi
  app_id="$(printf '%s' "${create}" | json_get id)"
  echo "remote E2E: assigning ${name} to VM pool" >&2
  configure_app_build_pool "${app_id}"
  echo "remote E2E: deploying ${name}" >&2
  deploy="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" \
    -X POST "${BASE_URL}/api/apps/${app_id}/deploy" \
    --data "{\"commitSha\":\"${FIXTURE_SHAS[${repo}]}\"}")"
  deployment_id="$(printf '%s' "${deploy}" | json_get deploymentId)"
  wait_deployment "${deployment_id}"
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  published="$(printf '%s' "${detail}" | json_get currentDeployment.publishedPort)"
  docker exec "${RUNNER_DIND}" wget -qO- "http://127.0.0.1:${published}${health}" >/dev/null
  if [ -n "$(docker -H "${BUILDER_DOCKER_HOST}" ps -aq --filter "label=hostlet.app_id=${app_id}")" ]; then
    echo "builder daemon ran an application container for ${app_id}" >&2
    return 1
  fi
  docker -H "${RUNNER_DOCKER_HOST}" ps -q --filter "label=hostlet.app_id=${app_id}" | grep -q .
  docker exec "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet -Atc \
    "select count(*) from agent_jobs where deployment_id='${deployment_id}' and job_type='build' and claimed_by='${BUILDER_ID}' and status='success'" | grep -q '^1$'
  docker exec "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet -Atc \
    "select count(*) from agent_jobs where deployment_id='${deployment_id}' and job_type='release' and server_id='00000000-0000-0000-0000-000000000001' and status='success'" | grep -q '^1$'
  docker exec "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet -Atc \
    "select count(*) from agent_jobs where deployment_id='${deployment_id}' and payload_json::text like '%must-not-cross-artifact-boundary%' and status='success'" | grep -q '^0$'
  printf '%s %s %s\n' "${app_id}" "${deployment_id}" "${published}"
}

start_registry() {
  local storage="$1"
  mkdir -p "${storage}"
  docker run -d --name "${REGISTRY_CONTAINER}" --network "${NETWORK}" --network-alias registry \
    --user "$(id -u):$(id -g)" \
    -v "${ROOT}/infra/zot-config.json:/etc/zot/config.json:ro" \
    -v "${TMP_DIR}/registry.htpasswd:/etc/zot/htpasswd:ro" \
    -v "${storage}:/var/lib/registry" \
    ghcr.io/project-zot/zot-linux-amd64:v2.1.18 serve /etc/zot/config.json >/dev/null
}

ensure_railpack
docker network create "${NETWORK}" >/dev/null
start_postgres_container postgres:16-alpine
wait_postgres_ready
POSTGRES_PORT="$(discover_postgres_port)"

docker run --rm httpd:2.4-alpine htpasswd -Bnb \
  "${REGISTRY_PUSH_USERNAME}" "${REGISTRY_PUSH_PASSWORD}" >"${TMP_DIR}/registry.htpasswd"
docker run --rm httpd:2.4-alpine htpasswd -Bnb \
  "${REGISTRY_PULL_USERNAME}" "${REGISTRY_PULL_PASSWORD}" >>"${TMP_DIR}/registry.htpasswd"
chmod 0600 "${TMP_DIR}/registry.htpasswd"

BRIDGE_GATEWAY="$(docker network inspect "${NETWORK}" -f '{{(index .IPAM.Config 0).Gateway}}')"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=Hostlet Remote E2E CA' \
  -keyout "${TMP_DIR}/ca.key" -out "${TMP_DIR}/ca.crt" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj "/CN=${BRIDGE_GATEWAY}" \
  -keyout "${TMP_DIR}/registry.key" -out "${TMP_DIR}/registry.csr" >/dev/null 2>&1
printf 'subjectAltName=IP:%s\nextendedKeyUsage=serverAuth\n' "${BRIDGE_GATEWAY}" >"${TMP_DIR}/registry.ext"
openssl x509 -req -days 1 -in "${TMP_DIR}/registry.csr" -CA "${TMP_DIR}/ca.crt" \
  -CAkey "${TMP_DIR}/ca.key" -CAcreateserial -extfile "${TMP_DIR}/registry.ext" \
  -out "${TMP_DIR}/registry.crt" >/dev/null 2>&1
cat >"${TMP_DIR}/Caddyfile" <<'CADDY'
:443 {
  tls /certs/registry.crt /certs/registry.key
  reverse_proxy registry:5000
}
CADDY
start_registry "${TMP_DIR}/registry-data"
docker run -d --name "${PROXY_CONTAINER}" --network "${NETWORK}" -p 0.0.0.0::443 \
  -v "${TMP_DIR}:/certs:ro" -v "${TMP_DIR}/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2.10-alpine >/dev/null
REGISTRY_PORT="$(docker port "${PROXY_CONTAINER}" 443/tcp | sed 's/.*://')"
REGISTRY_HOST="${BRIDGE_GATEWAY}:${REGISTRY_PORT}"
REGISTRY_URL="https://${REGISTRY_HOST}"
mkdir -p "${TMP_DIR}/docker-certs/${REGISTRY_HOST}"
cp "${TMP_DIR}/ca.crt" "${TMP_DIR}/docker-certs/${REGISTRY_HOST}/ca.crt"

for daemon in "${RUNNER_DIND}" "${BUILDER_DIND}"; do
  docker run -d --privileged --name "${daemon}" --network "${NETWORK}" \
    -e DOCKER_TLS_CERTDIR= \
    --mount "type=bind,src=${TMP_DIR}/docker-certs/${REGISTRY_HOST}/ca.crt,dst=/etc/docker/certs.d/${REGISTRY_HOST}/ca.crt,readonly" \
    -p 127.0.0.1::2375 docker:29-dind --host=tcp://0.0.0.0:2375 --tls=false >/dev/null
done
RUNNER_DOCKER_HOST="tcp://127.0.0.1:$(docker port "${RUNNER_DIND}" 2375/tcp | sed 's/.*://')"
BUILDER_DOCKER_HOST="tcp://127.0.0.1:$(docker port "${BUILDER_DIND}" 2375/tcp | sed 's/.*://')"
for host in "${RUNNER_DOCKER_HOST}" "${BUILDER_DOCKER_HOST}"; do
  for _ in $(seq 1 90); do docker -H "${host}" info >/dev/null 2>&1 && break; sleep 1; done
  docker -H "${host}" info >/dev/null
done
make_fixture_repo dockerfile "${ROOT}/scripts/fixtures/generated-apps/dockerfile"
make_fixture_repo railpack "${ROOT}/scripts/fixtures/generated-apps/node"
make_fixture_repo compose "${ROOT}/scripts/fixtures/generated-apps/compose"
make_fixture_repo topology "${ROOT}/scripts/fixtures/generated-apps/topology-patchwork"
cat >"${GIT_CONFIG_GLOBAL}" <<EOF
[url "file://${TMP_DIR}/git/"]
	insteadOf = https://github.com/hostlet-ci/
[safe]
	directory = *
EOF

export_self_hosted_env "${POSTGRES_PORT}" "${API_PORT}"
export BIND_ADDR="0.0.0.0:${API_PORT}"
export HOSTLET_ARTIFACT_REGISTRY_URL="${REGISTRY_URL}"
export HOSTLET_ARTIFACT_REGISTRY_INTERNAL_URL="${REGISTRY_URL}"
export HOSTLET_ARTIFACT_REGISTRY_PUSH_USERNAME="${REGISTRY_PUSH_USERNAME}"
export HOSTLET_ARTIFACT_REGISTRY_PUSH_PASSWORD="${REGISTRY_PUSH_PASSWORD}"
export HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME="${REGISTRY_PULL_USERNAME}"
export HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD="${REGISTRY_PULL_PASSWORD}"
export HOSTLET_AGENT_IMAGE=hostlet-agent-e2e
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMP_DIR}/target}"
export GIT_CONFIG_GLOBAL
ci_build_binary hostlet-api hostlet-api
ci_build_binary hostlet-agent hostlet-agent
AGENT_BINARY="$(realpath "$(ci_binary_path hostlet-agent)")"
"$(ci_binary_path hostlet-api)" >"${API_LOG}" 2>&1 & API_PID="$!"
BASE_URL="http://127.0.0.1:${API_PORT}"
ORIGIN="http://127.0.0.1:3000"
ORIGIN_CSRF=(-H "origin: ${ORIGIN}" -H "x-hostlet-csrf: 1")
JSON_CT=(-H "content-type: application/json")
wait_api
expect_status 204 -c "${COOKIE_JAR}" -X POST "${BASE_URL}/api/setup" "${ORIGIN_CSRF[@]}" \
  "${JSON_CT[@]}" -H "x-hostlet-setup-token: ${HOSTLET_SETUP_TOKEN}" \
  --data '{"password":"ci-self-hosted-password"}'
USER_ID=00000000-0000-0000-0000-000000000101
docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
INSERT INTO users (id,github_id,login) VALUES ('${USER_ID}',9001,'ci-user') ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login;
UPDATE servers SET max_concurrent_apps=16 WHERE id='00000000-0000-0000-0000-000000000001';
SQL
UNLOCK_COOKIE="$(awk '$6 == "hostlet_unlock" {print $7}' "${COOKIE_JAR}" | tail -1)"
AUTH_COOKIE="hostlet_unlock=${UNLOCK_COOKIE}; hostlet_session=$(signed_cookie "${USER_ID}")"

docker run -d --name "${RUNNER_AGENT_CONTAINER}" --network "container:${RUNNER_DIND}" \
  -v "${AGENT_BINARY}:/usr/local/bin/hostlet-agent:ro" \
  -v "$(command -v docker):/usr/local/bin/docker:ro" \
  -v /usr/libexec/docker/cli-plugins/docker-buildx:/usr/libexec/docker/cli-plugins/docker-buildx:ro \
  -v /usr/libexec/docker/cli-plugins/docker-compose:/usr/libexec/docker/cli-plugins/docker-compose:ro \
  -v "${HOSTLET_RAILPACK_BIN:-/usr/local/bin/railpack}:/usr/local/bin/railpack:ro" \
  -v "${TMP_DIR}:${TMP_DIR}" \
  -e DOCKER_HOST=tcp://127.0.0.1:2375 -e HOSTLET_API_URL="http://${BRIDGE_GATEWAY}:${API_PORT}" \
  -e HOSTLET_SERVER_ID=00000000-0000-0000-0000-000000000001 -e HOSTLET_AGENT_TOKEN="${LOCAL_AGENT_TOKEN}" \
  -e HOSTLET_JOB_SIGNING_SECRET="${JOB_SIGNING_SECRET}" -e HOSTLET_WORKDIR="${TMP_DIR}/runner-work" \
  -e HOSTLET_LOCAL_MODE=true -e HOSTLET_HEALTH_HOST=127.0.0.1 -e HOSTLET_LOCAL_ROUTER=caddy \
  -e HOSTLET_LOCAL_ROUTER_SNIPPETS_DIR="${TMP_DIR}/caddy" -e HOSTLET_LOCAL_ROUTER_RELOAD=true \
  -e HOSTLET_EXTRA_CA_CERT_PATH="${TMP_DIR}/ca.crt" -e SSL_CERT_FILE="${TMP_DIR}/ca.crt" \
  -e GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL}" ubuntu:24.04 \
  sh -c 'apt-get update >/dev/null && apt-get install -y --no-install-recommends ca-certificates git >/dev/null && exec hostlet-agent' >/dev/null
wait_agent /api/servers 00000000-0000-0000-0000-000000000001

POOL="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" \
  -X POST "${BASE_URL}/api/build-pools" --data '{"name":"remote-e2e","provider":"vm","maxConcurrentBuilds":1,"supportedPlatforms":["linux/amd64"]}')"
POOL_ID="$(printf '%s' "${POOL}" | json_get id)"
ENROLLMENT="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" \
  -X POST "${BASE_URL}/api/build-pools/${POOL_ID}/enrollments" --data '{}')"
TOKEN="$(printf '%s' "${ENROLLMENT}" | json_get token)"
BUILDER="$(curl -fsS "${JSON_CT[@]}" -X POST "${BASE_URL}/api/agent/builders/register" \
  --data "{\"enrollmentToken\":\"${TOKEN}\",\"name\":\"remote-e2e-builder\",\"platforms\":[\"linux/amd64\"],\"maxConcurrentBuilds\":1}")"
BUILDER_ID="$(printf '%s' "${BUILDER}" | json_get serverId)"
BUILDER_TOKEN="$(printf '%s' "${BUILDER}" | json_get agentToken)"
BUILDER_SIGNING="$(printf '%s' "${BUILDER}" | json_get jobSigningSecret)"
docker run -d --name "${BUILDER_AGENT_CONTAINER}" --network "container:${BUILDER_DIND}" \
  -v "${AGENT_BINARY}:/usr/local/bin/hostlet-agent:ro" \
  -v "$(command -v docker):/usr/local/bin/docker:ro" \
  -v /usr/libexec/docker/cli-plugins/docker-buildx:/usr/libexec/docker/cli-plugins/docker-buildx:ro \
  -v /usr/libexec/docker/cli-plugins/docker-compose:/usr/libexec/docker/cli-plugins/docker-compose:ro \
  -v "${HOSTLET_RAILPACK_BIN:-/usr/local/bin/railpack}:/usr/local/bin/railpack:ro" \
  -v "${TMP_DIR}:${TMP_DIR}" \
  -e DOCKER_HOST=tcp://127.0.0.1:2375 -e HOSTLET_API_URL="http://${BRIDGE_GATEWAY}:${API_PORT}" \
  -e HOSTLET_SERVER_ID="${BUILDER_ID}" -e HOSTLET_AGENT_TOKEN="${BUILDER_TOKEN}" \
  -e HOSTLET_JOB_SIGNING_SECRET="${BUILDER_SIGNING}" -e HOSTLET_WORKDIR="${TMP_DIR}/builder-work" \
  -e HOSTLET_LOCAL_MODE=true -e HOSTLET_HEALTH_HOST=127.0.0.1 -e HOSTLET_LOCAL_ROUTER=caddy \
  -e HOSTLET_LOCAL_ROUTER_SNIPPETS_DIR="${TMP_DIR}/builder-caddy" -e HOSTLET_LOCAL_ROUTER_RELOAD=true \
  -e HOSTLET_EXTRA_CA_CERT_PATH="${TMP_DIR}/ca.crt" -e SSL_CERT_FILE="${TMP_DIR}/ca.crt" \
  -e HOSTLET_RAILPACK_BUILDKIT_PRIVILEGED=true -e HOSTLET_RAILPACK_BUILDKIT_CONTAINER="remote-buildkit-${RUN_ID}" \
  -e HOSTLET_RAILPACK_BIN=/usr/local/bin/railpack -e GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL}" \
  ubuntu:24.04 \
  sh -c 'apt-get update >/dev/null && apt-get install -y --no-install-recommends ca-certificates git >/dev/null && exec hostlet-agent' >/dev/null
wait_agent /api/builders "${BUILDER_ID}"

read -r DOCKER_APP DOCKER_DEPLOY DOCKER_PORT < <(create_and_deploy remote-dockerfile dockerfile single '{}')
docker exec "${RUNNER_DIND}" wget -qO- "http://127.0.0.1:${DOCKER_PORT}/" | grep -q hostlet-remote-dockerfile
create_and_deploy remote-railpack railpack single '{}' >/dev/null
create_and_deploy remote-compose compose compose '{}' >/dev/null
create_and_deploy remote-addons railpack single '{"compose":{"addOns":[{"key":"postgres"}]}}' >/dev/null
create_and_deploy remote-topology topology compose '{"generatedTopology":{"schemaVersion":1,"mode":"auto","backendPathPrefixes":["/api","/socket.io"]}}' 80 / >/dev/null

expect_status 200 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${DOCKER_APP}"
docker stop "${REGISTRY_CONTAINER}" >/dev/null
docker exec "${RUNNER_DIND}" wget -qO- "http://127.0.0.1:${DOCKER_PORT}/" | grep -q hostlet-remote-dockerfile
docker rm "${REGISTRY_CONTAINER}" >/dev/null
start_registry "${TMP_DIR}/registry-data-rebuilt"
registry_ready=0
for _ in $(seq 1 30); do
  if curl -fsS --cacert "${TMP_DIR}/ca.crt" \
    --user "${REGISTRY_PULL_USERNAME}:${REGISTRY_PULL_PASSWORD}" \
    "${REGISTRY_URL}/v2/" >/dev/null 2>&1; then
    registry_ready=1
    break
  fi
  sleep 1
done
[ "${registry_ready}" = "1" ]
REBUILD="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" \
  -X POST "${BASE_URL}/api/apps/${DOCKER_APP}/deploy" --data "{\"commitSha\":\"${FIXTURE_SHAS[dockerfile]}\"}")"
wait_deployment "$(printf '%s' "${REBUILD}" | json_get deploymentId)"

expect_status 200 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${DOCKER_APP}"
echo "isolated remote builder -> TLS zot -> runner E2E passed"
