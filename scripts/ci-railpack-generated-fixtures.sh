#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RAILPACK_BIN="${HOSTLET_RAILPACK_BIN:-railpack}"
RUN_ID="${GITHUB_RUN_ID:-local}-$$"
BUILDKIT_CONTAINER="${HOSTLET_RAILPACK_BUILDKIT_CONTAINER:-hostlet-railpack-buildkit-ci-${RUN_ID}}"
BUILDKIT_IMAGE="${HOSTLET_RAILPACK_BUILDKIT_IMAGE:-moby/buildkit:buildx-stable-1}"
TMP_DIR="$(mktemp -d "/tmp/hostlet-railpack-fixtures-${RUN_ID}.XXXXXX")"
METRICS_FILE="${HOSTLET_RAILPACK_METRICS_FILE:-${TMP_DIR}/metrics.json}"
METRICS_EVENTS="${TMP_DIR}/metrics.ndjson"

cleanup() {
  docker ps -aq --filter "name=hostlet-railpack-fixture-${RUN_ID}" | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker images "hostlet/railpack-fixture-*" --format "{{.Repository}}:{{.Tag}}" \
    | grep ":${RUN_ID}$" \
    | xargs -r docker image rm -f >/dev/null 2>&1 || true
  docker rm -f "${BUILDKIT_CONTAINER}" >/dev/null 2>&1 || true
  rm -rf "${TMP_DIR}"
}

if ! command -v "${RAILPACK_BIN}" >/dev/null 2>&1; then
  echo "Railpack binary not found: ${RAILPACK_BIN}" >&2
  exit 1
fi

docker run -d --name "${BUILDKIT_CONTAINER}" --privileged "${BUILDKIT_IMAGE}" >/dev/null
export BUILDKIT_HOST="docker-container://${BUILDKIT_CONTAINER}"
mkdir -p "$(dirname "${METRICS_FILE}")"
printf '[]\n' >"${METRICS_FILE}"

write_metrics_file() {
  local tmp="${METRICS_FILE}.tmp"
  local first=1
  {
    printf '[\n'
    if [ -f "${METRICS_EVENTS}" ]; then
      while IFS= read -r metric; do
        if [ -z "${metric}" ]; then
          continue
        fi
        if [ "${first}" = "0" ]; then
          printf ',\n'
        fi
        first=0
        printf '  %s' "${metric}"
      done <"${METRICS_EVENTS}"
    fi
    printf '\n]\n'
  } >"${tmp}"
  mv "${tmp}" "${METRICS_FILE}"
}

record_metric() {
  local name="$1"
  local image_bytes="$2"
  local max_image_bytes="$3"
  local build_seconds="$4"
  local max_build_seconds="$5"
  local health_seconds="$6"
  local max_health_seconds="$7"
  printf '{"fixture":"%s","imageBytes":%s,"maxImageBytes":%s,"buildSeconds":%s,"maxBuildSeconds":%s,"bootSeconds":%s,"maxBootSeconds":%s,"healthSeconds":%s,"maxHealthSeconds":%s}\n' \
    "${name}" "${image_bytes}" "${max_image_bytes}" "${build_seconds}" "${max_build_seconds}" "${health_seconds}" "${max_health_seconds}" "${health_seconds}" "${max_health_seconds}" >>"${METRICS_EVENTS}"
  write_metrics_file
}
trap 'write_metrics_file; cleanup' EXIT

run_fixture() {
  local name="$1"
  local port="$2"
  local health_path="$3"
  local expected="$4"
  local max_image_bytes="$5"
  local max_build_seconds="$6"
  local max_health_seconds="$7"
  local image="hostlet/railpack-fixture-${name}:${RUN_ID}"
  local container="hostlet-railpack-fixture-${RUN_ID}-${name}"
  local fixture="${ROOT}/scripts/fixtures/generated-apps/${name}"
  local build_start build_end build_seconds health_start health_end health_seconds image_bytes

  build_start="$(date +%s)"
  "${RAILPACK_BIN}" plan --error-missing-start --env "PORT=${port}" "${fixture}" >"${TMP_DIR}/railpack-plan-${name}.txt"
  "${RAILPACK_BIN}" build --name "${image}" --progress plain --cache-key "hostlet-${name}" --env "PORT=${port}" --error-missing-start "${fixture}"
  build_end="$(date +%s)"
  build_seconds="$((build_end - build_start))"

  health_start="$(date +%s)"
  docker run -d --name "${container}" -e "PORT=${port}" -p "127.0.0.1::${port}" "${image}" >/dev/null
  local published
  published="$(docker inspect -f "{{(index (index .NetworkSettings.Ports \"${port}/tcp\") 0).HostPort}}" "${container}")"
  for _ in $(seq 1 60); do
    if curl -fsS "http://127.0.0.1:${published}${health_path}" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  curl -fsS "http://127.0.0.1:${published}${health_path}" >/dev/null
  health_end="$(date +%s)"
  health_seconds="$((health_end - health_start))"
  curl -fsS "http://127.0.0.1:${published}/" | grep -q "${expected}"
  image_bytes="$(docker image inspect -f "{{.Size}}" "${image}")"
  record_metric "${name}" "${image_bytes}" "${max_image_bytes}" "${build_seconds}" "${max_build_seconds}" "${health_seconds}" "${max_health_seconds}"
  echo "railpack fixture ${name} image bytes=${image_bytes} max=${max_image_bytes} build_seconds=${build_seconds} max=${max_build_seconds} health_seconds=${health_seconds} max=${max_health_seconds}"
  if [ "${image_bytes}" -gt "${max_image_bytes}" ]; then
    echo "railpack fixture ${name} image size exceeded budget: ${image_bytes} > ${max_image_bytes}" >&2
    exit 1
  fi
  if [ "${build_seconds}" -gt "${max_build_seconds}" ]; then
    echo "railpack fixture ${name} build time exceeded budget: ${build_seconds}s > ${max_build_seconds}s" >&2
    exit 1
  fi
  if [ "${health_seconds}" -gt "${max_health_seconds}" ]; then
    echo "railpack fixture ${name} health time exceeded budget: ${health_seconds}s > ${max_health_seconds}s" >&2
    exit 1
  fi
  docker rm -f "${container}" >/dev/null
}

run_fixture python 3000 /health hostlet-generated-python 150000000 180 60
run_fixture go 3000 /health hostlet-generated-go 60000000 180 60
run_fixture rust 3000 /health hostlet-generated-rust 60000000 180 60
run_fixture static 3000 / hostlet-generated-static 80000000 180 60
run_fixture node 3000 /health hostlet-ci 180000000 180 60
run_fixture bun 3000 /health hostlet-generated-bun 220000000 180 60
run_fixture yarn 3000 /health hostlet-generated-yarn 180000000 180 60
run_fixture next-pnpm 3000 / hostlet-generated-next-pnpm 320000000 240 60

echo "Railpack generated fixture builds passed"
echo "Railpack generated fixture metrics: ${METRICS_FILE}"
