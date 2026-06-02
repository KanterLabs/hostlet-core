#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

# Export once so every cargo step below shares the same target dir.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}"

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
docker compose -f infra/docker-compose.yml config >/dev/null
HOSTLET_IMAGE_TAG="${HOSTLET_IMAGE_TAG:-v0.0.0}" docker compose -f infra/docker-compose.prod.yml config >/dev/null
