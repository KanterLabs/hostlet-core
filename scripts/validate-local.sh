#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

# Export once so every cargo step below shares the same target dir.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/hostlet-target-local-$$}"

# Print a labeled banner before each gate so a failure is easy to attribute
# without scrolling back through cargo/pnpm output.
step() {
  echo "==> $1"
}

step "git whitespace check"
git diff --check

step "line-cap guard"
scripts/check-line-cap.sh
scripts/check-line-cap-selftest.sh

step "cargo fmt"
cargo fmt --all -- --check

step "cargo clippy"
cargo clippy --workspace --all-targets --all-features -- -D warnings

step "cargo test"
cargo test --workspace

step "web install"
pnpm --dir apps/web install --frozen-lockfile

step "web lint"
pnpm --dir apps/web lint

step "web build"
pnpm --dir apps/web build

step "compose config"
ci_test_secret_value() {
  printf 'ci-test-%s-value-000001' "$1"
}

compose_config() {
  POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-ci-only-not-a-secret-postgres}" \
  PUBLIC_API_URL="${PUBLIC_API_URL:-http://localhost:8080}" \
  PUBLIC_WEB_URL="${PUBLIC_WEB_URL:-http://localhost:3000}" \
  HOSTLET_SETUP_TOKEN="${HOSTLET_SETUP_TOKEN:-$(ci_test_secret_value setup-token)}" \
  HOSTLET_ALLOWED_GITHUB_LOGINS="${HOSTLET_ALLOWED_GITHUB_LOGINS:-ci-user}" \
  ENCRYPTION_KEY="${ENCRYPTION_KEY:-YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=}" \
  JOB_SIGNING_SECRET="${JOB_SIGNING_SECRET:-$(ci_test_secret_value job-signing)}" \
  SESSION_SECRET="${SESSION_SECRET:-$(ci_test_secret_value session-secret)}" \
  LOCAL_AGENT_TOKEN="${LOCAL_AGENT_TOKEN:-$(ci_test_secret_value local-agent-token)}" \
  GITHUB_WEBHOOK_SECRET="${GITHUB_WEBHOOK_SECRET:-$(ci_test_secret_value webhook-secret)}" \
  DOCKER_GID="${DOCKER_GID:-998}" \
  HOSTLET_IMAGE_TAG="${HOSTLET_IMAGE_TAG:-v0.0.0}" \
  HOSTLET_API_IMAGE="${HOSTLET_API_IMAGE:-ghcr.io/shanekanterman04/hostlet-api@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}" \
  HOSTLET_WEB_IMAGE="${HOSTLET_WEB_IMAGE:-ghcr.io/shanekanterman04/hostlet-web@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}" \
  HOSTLET_AGENT_IMAGE="${HOSTLET_AGENT_IMAGE:-ghcr.io/shanekanterman04/hostlet-agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}" \
  HOSTLET_SCREENSHOTTER_IMAGE="${HOSTLET_SCREENSHOTTER_IMAGE:-ghcr.io/shanekanterman04/hostlet-screenshotter@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}" \
  docker compose -f "$1" config >/dev/null
}
compose_config infra/docker-compose.yml
compose_config infra/docker-compose.prod.yml
