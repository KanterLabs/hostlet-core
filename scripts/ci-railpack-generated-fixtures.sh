#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RAILPACK_BIN="${HOSTLET_RAILPACK_BIN:-railpack}"
BUILDKIT_CONTAINER="${HOSTLET_RAILPACK_BUILDKIT_CONTAINER:-hostlet-railpack-buildkit-ci}"
BUILDKIT_IMAGE="${HOSTLET_RAILPACK_BUILDKIT_IMAGE:-moby/buildkit:buildx-stable-1}"
RUN_ID="${GITHUB_RUN_ID:-local}-$$"

cleanup() {
  docker ps -aq --filter "name=hostlet-railpack-fixture-${RUN_ID}" | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker images "hostlet/railpack-fixture-*" --format "{{.Repository}}:{{.Tag}}" \
    | grep ":${RUN_ID}$" \
    | xargs -r docker image rm -f >/dev/null 2>&1 || true
  docker rm -f "${BUILDKIT_CONTAINER}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if ! command -v "${RAILPACK_BIN}" >/dev/null 2>&1; then
  echo "Railpack binary not found: ${RAILPACK_BIN}" >&2
  exit 1
fi

docker run -d --name "${BUILDKIT_CONTAINER}" --privileged "${BUILDKIT_IMAGE}" >/dev/null
export BUILDKIT_HOST="docker-container://${BUILDKIT_CONTAINER}"

run_fixture() {
  local name="$1"
  local port="$2"
  local health_path="$3"
  local expected="$4"
  local image="hostlet/railpack-fixture-${name}:${RUN_ID}"
  local container="hostlet-railpack-fixture-${RUN_ID}-${name}"
  local fixture="${ROOT}/scripts/fixtures/generated-apps/${name}"

  "${RAILPACK_BIN}" plan --error-missing-start --env "PORT=${port}" "${fixture}" >/tmp/hostlet-railpack-plan-"${name}".txt
  "${RAILPACK_BIN}" build --name "${image}" --progress plain --cache-key "hostlet-${name}" --env "PORT=${port}" --error-missing-start "${fixture}"

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
  curl -fsS "http://127.0.0.1:${published}/" | grep -q "${expected}"
  docker image inspect -f "railpack fixture ${name} image bytes={{.Size}}" "${image}"
  docker rm -f "${container}" >/dev/null
}

run_fixture python 3000 /health hostlet-generated-python
run_fixture go 3000 /health hostlet-generated-go
run_fixture rust 3000 /health hostlet-generated-rust
run_fixture static 3000 / hostlet-generated-static
run_fixture next-pnpm 3000 / hostlet-generated-next-pnpm

echo "Railpack generated fixture builds passed"
