#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/ci-metrics-lib.sh
source "${ROOT}/scripts/ci-metrics-lib.sh"
RAILPACK_BIN="${HOSTLET_RAILPACK_BIN:-railpack}"
RUN_ID="${GITHUB_RUN_ID:-local}-$$"
BUILDKIT_CONTAINER="${HOSTLET_RAILPACK_BUILDKIT_CONTAINER:-hostlet-railpack-buildkit-ci-${RUN_ID}}"
BUILDKIT_IMAGE="${HOSTLET_RAILPACK_BUILDKIT_IMAGE:-moby/buildkit:buildx-stable-1}"
TMP_DIR="$(mktemp -d "/tmp/hostlet-railpack-fixtures-${RUN_ID}.XXXXXX")"
METRICS_FILE="${HOSTLET_RAILPACK_METRICS_FILE:-${TMP_DIR}/metrics.json}"

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
ci_metrics_init "${METRICS_FILE}"

record_metric() {
  local name="$1"
  local image_bytes="$2"
  local max_image_bytes="$3"
  local max_build_seconds="$4"
  local max_health_seconds="$5"
  local plan_duration_ms="$6"
  local railpack_build_duration_ms="$7"
  local build_duration_ms="$8"
  local container_start_duration_ms="$9"
  local health_probe_duration_ms="${10}"
  local boot_duration_ms="${11}"
  local health_attempts="${12}"
  local ready_stats_json="${13}"
  python3 - \
    "${name}" \
    "${image_bytes}" \
    "${max_image_bytes}" \
    "${max_build_seconds}" \
    "${max_health_seconds}" \
    "${plan_duration_ms}" \
    "${railpack_build_duration_ms}" \
    "${build_duration_ms}" \
    "${container_start_duration_ms}" \
    "${health_probe_duration_ms}" \
    "${boot_duration_ms}" \
    "${health_attempts}" \
    "${ready_stats_json}" <<'PY' | ci_metrics_append_object "${METRICS_FILE}"
import json
import sys

(
    fixture,
    image_bytes,
    max_image_bytes,
    max_build_seconds,
    max_health_seconds,
    plan_duration_ms,
    railpack_build_duration_ms,
    build_duration_ms,
    container_start_duration_ms,
    health_probe_duration_ms,
    boot_duration_ms,
    health_attempts,
    ready_stats_json,
) = sys.argv[1:]

build_ms = int(build_duration_ms)
boot_ms = int(boot_duration_ms)
payload = {
    "fixture": fixture,
    "scenario": "railpackGeneratedFixture",
    "imageBytes": int(image_bytes),
    "maxImageBytes": int(max_image_bytes),
    "buildSeconds": build_ms // 1000,
    "maxBuildSeconds": int(max_build_seconds),
    "bootSeconds": boot_ms // 1000,
    "maxBootSeconds": int(max_health_seconds),
    "healthSeconds": boot_ms // 1000,
    "maxHealthSeconds": int(max_health_seconds),
    "planDurationMs": int(plan_duration_ms),
    "railpackBuildDurationMs": int(railpack_build_duration_ms),
    "buildDurationMs": build_ms,
    "containerStartDurationMs": int(container_start_duration_ms),
    "healthProbeDurationMs": int(health_probe_duration_ms),
    "bootDurationMs": boot_ms,
    "healthAttempts": int(health_attempts),
}
payload.update(json.loads(ready_stats_json))
print(json.dumps(payload, sort_keys=True))
PY
}
trap cleanup EXIT

now_ms() {
  date +%s%3N
}

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
  local plan_start plan_end railpack_build_start railpack_build_end build_duration_ms build_seconds
  local container_start container_end health_probe_start health_probe_end boot_duration_ms health_seconds
  local image_bytes health_attempts ready_stats_json

  plan_start="$(now_ms)"
  "${RAILPACK_BIN}" plan --error-missing-start --env "PORT=${port}" "${fixture}" >"${TMP_DIR}/railpack-plan-${name}.txt"
  plan_end="$(now_ms)"
  railpack_build_start="$(now_ms)"
  "${RAILPACK_BIN}" build --name "${image}" --progress plain --cache-key "hostlet-${name}" --env "PORT=${port}" --error-missing-start "${fixture}"
  railpack_build_end="$(now_ms)"
  build_duration_ms="$((railpack_build_end - plan_start))"
  build_seconds="$((build_duration_ms / 1000))"

  container_start="$(now_ms)"
  docker run -d --name "${container}" -e "PORT=${port}" -p "127.0.0.1::${port}" "${image}" >/dev/null
  local published
  published="$(docker inspect -f "{{(index (index .NetworkSettings.Ports \"${port}/tcp\") 0).HostPort}}" "${container}")"
  container_end="$(now_ms)"
  health_probe_start="$(now_ms)"
  health_attempts=0
  for _ in $(seq 1 60); do
    health_attempts="$((health_attempts + 1))"
    if curl -fsS "http://127.0.0.1:${published}${health_path}" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  curl -fsS "http://127.0.0.1:${published}${health_path}" >/dev/null
  health_probe_end="$(now_ms)"
  boot_duration_ms="$((health_probe_end - container_start))"
  health_seconds="$((boot_duration_ms / 1000))"
  curl -fsS "http://127.0.0.1:${published}/" | grep -q "${expected}"
  image_bytes="$(docker image inspect -f "{{.Size}}" "${image}")"
  ready_stats_json="$(ci_docker_ready_stats_json "${container}")"
  record_metric \
    "${name}" \
    "${image_bytes}" \
    "${max_image_bytes}" \
    "${max_build_seconds}" \
    "${max_health_seconds}" \
    "$((plan_end - plan_start))" \
    "$((railpack_build_end - railpack_build_start))" \
    "${build_duration_ms}" \
    "$((container_end - container_start))" \
    "$((health_probe_end - health_probe_start))" \
    "${boot_duration_ms}" \
    "${health_attempts}" \
    "${ready_stats_json}"
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
