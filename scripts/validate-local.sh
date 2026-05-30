#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

git diff --check
scripts/check-line-cap.sh
scripts/check-line-cap-selftest.sh
cargo fmt --all -- --check
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}" cargo clippy --workspace --all-targets --all-features -- -D warnings
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/hostlet-target}" cargo test --workspace
pnpm --dir apps/web install --frozen-lockfile
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config >/dev/null
HOSTLET_IMAGE_TAG="${HOSTLET_IMAGE_TAG:-v0.0.0}" docker compose -f infra/docker-compose.prod.yml config >/dev/null
