#!/usr/bin/env bash
set -euo pipefail

# Pre-warms the persistent self-hosted Core runner's caches so CI does not
# re-download them on every run: the pnpm store, the Playwright chromium
# browser, and the base Docker images that Core's builds FROM. Mirrors
# hostlet-cloud/scripts/ci-cloud-runner-prewarm.sh. Idempotent and safe to run
# from a schedule.

PNPM_VERSION="${HOSTLET_CORE_PNPM_VERSION:-10.33.2}"
PLAYWRIGHT_CACHE="${PLAYWRIGHT_BROWSERS_PATH:-${HOME}/.cache/ms-playwright}"
PNPM_STORE="${PNPM_HOME:-${HOME}/.local/share/pnpm}"

if ! command -v node >/dev/null 2>&1; then
  echo "node is required to prewarm core runner browser tooling" >&2
  exit 1
fi

if ! command -v pnpm >/dev/null 2>&1; then
  corepack enable
  corepack prepare "pnpm@${PNPM_VERSION}" --activate
fi

run_pnpm() {
  if command -v pnpm >/dev/null 2>&1 && pnpm --version >/dev/null 2>&1; then
    pnpm "$@"
    return
  fi
  npx --yes "pnpm@${PNPM_VERSION}" "$@"
}

tmp_dir="$(mktemp -d "${RUNNER_TEMP:-/tmp}/hostlet-core-prewarm-${GITHUB_RUN_ID:-local}-$$.XXXXXX")"
cleanup() {
  rm -rf "${tmp_dir}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -p "${PLAYWRIGHT_CACHE}" "${PNPM_STORE}"
cd "${tmp_dir}"
printf '{"private":true,"devDependencies":{"@playwright/test":"1.60.0"}}\n' > package.json
run_pnpm install --lockfile=false
run_pnpm exec playwright install chromium

# Base images that Core's Dockerfiles/CI build FROM or run. Kept warm so the
# cleanup (which keeps base images) plus this prewarm means no cold multi-GB
# re-pulls. `docker image inspect` first makes the pull a no-op when present.
for image in \
  zricethezav/gitleaks:v8.30.1 \
  postgres:16-alpine \
  moby/buildkit:buildx-stable-1 \
  node:22-alpine \
  node:22-bookworm-slim \
  rust:1-bookworm \
  mcr.microsoft.com/playwright:v1.60.0-noble; do
  docker image inspect "${image}" >/dev/null 2>&1 || docker pull "${image}" >/dev/null
done

if ! find "${PLAYWRIGHT_CACHE}" -mindepth 1 -maxdepth 1 -type d | grep -q .; then
  echo "Playwright browser cache is empty after prewarm: ${PLAYWRIGHT_CACHE}" >&2
  exit 1
fi

echo "core runner prewarm passed"
