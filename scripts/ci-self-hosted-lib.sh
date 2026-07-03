#!/usr/bin/env bash
# Shared helpers for the self-hosted CI smoke / E2E scripts
# (ci-self-hosted-api-smoke.sh and ci-self-hosted-deploy-e2e.sh).
#
# CROSS-REPO CONTRACT: this file is also sourced by hostlet-cloud scripts via
# vendor/hostlet-core/scripts/. Any change to function signatures or semantics
# (pick_local_port, ci_cargo, ci_build_binary, etc.) is a breaking change for
# cloud CI. Update callers in both repos together when modifying these APIs.
#
# This file is meant to be *sourced*, not executed. The functions below rely on
# variables the caller exports/sets at runtime:
#   - signed_cookie:  ${SESSION_SECRET}
#   - expect_status:  ${TMP_DIR}
#   - bootstrap helpers: ${POSTGRES_CONTAINER}, ${RUN_ID}
# Keeping these here removes a large byte-for-byte copy-paste surface that
# previously lived in both scripts.

ensure_rust_toolchain_path() {
  local cargo_bin="${CARGO_HOME:-${HOME}/.cargo}/bin"
  case ":${PATH}:" in
    *":${cargo_bin}:"*) ;;
    *) export PATH="${cargo_bin}:${PATH}" ;;
  esac

  export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
}

ensure_rust_toolchain_path

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"

ci_cargo() {
  local cargo_bin
  cargo_bin="$(command -v cargo 2>/dev/null || true)"
  if [ -z "${cargo_bin}" ]; then
    cargo_bin="${CARGO_HOME:-${HOME}/.cargo}/bin/cargo"
  fi
  "${cargo_bin}" "$@"
}

ci_binary_path() {
  local binary="$1"
  printf '%s/debug/%s' "${CARGO_TARGET_DIR:-target}" "${binary}"
}

ci_build_binary() {
  local package="$1"
  local binary="$2"
  ci_cargo build -p "${package}" --bin "${binary}"
}

ci_tmp_dir() {
  local name="$1"
  local run_id="$2"
  local parent="${RUNNER_TEMP:-/tmp}"
  mkdir -p "${parent}"
  mktemp -d "${parent}/${name}-${run_id}.XXXXXX"
}

# json_get <dotted.path>: read JSON from stdin and print the value at the path,
# exiting non-zero if any segment is missing/null.
json_get() {
  node -e "let s=''; process.stdin.on('data', d => s += d); process.stdin.on('end', () => { const path = process.argv[1].split('.'); let v = JSON.parse(s); for (const key of path) v = v?.[key]; if (v === undefined || v === null) process.exit(2); process.stdout.write(String(v)); });" "$1"
}

# signed_cookie <value>: mint a v2 HMAC-signed session cookie for the given value
# using ${SESSION_SECRET}, valid for one hour.
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

# expect_status <expected-code> <curl-args...>: run curl, capture body to
# ${TMP_DIR}/response.txt, and fail if the HTTP status differs from expected.
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

pick_local_port() {
  python3 - <<'PY'
import socket
with socket.socket() as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

# start_postgres_container <image>: launch a throwaway Postgres bound to a random
# loopback port for ${POSTGRES_CONTAINER}.
start_postgres_container() {
  docker run -d --name "${POSTGRES_CONTAINER}" \
    -e POSTGRES_USER=hostlet \
    -e POSTGRES_PASSWORD=ci-only-not-a-secret-postgres \
    -e POSTGRES_DB=hostlet \
    -p 127.0.0.1::5432 \
    "$1" >/dev/null
}

# wait_postgres_ready: poll until the container accepts connections (up to ~60s).
wait_postgres_ready() {
  for _ in $(seq 1 60); do
    if docker exec "${POSTGRES_CONTAINER}" pg_isready -U hostlet -d hostlet >/dev/null 2>&1 &&
      docker exec "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet -c 'select 1' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 0
}

# discover_postgres_port: print the host port mapped to the container's 5432,
# failing if it cannot be discovered.
discover_postgres_port() {
  local port
  port="$(docker port "${POSTGRES_CONTAINER}" 5432/tcp | sed 's/.*://')"
  if [ -z "${port}" ]; then
    echo "Could not discover mapped Postgres port" >&2
    exit 1
  fi
  printf '%s' "${port}"
}

# export_self_hosted_env <postgres-port> <api-port>: export the shared self-hosted
# API configuration (mode, database URL, bind/public URLs, and the CI-only secret
# set) used by both scripts. Caller adds any script-specific exports afterward.
export_self_hosted_env() {
  local postgres_port="$1"
  local api_port="$2"
  export HOSTLET_MODE=self_hosted
  export DATABASE_URL="postgres://hostlet:ci-only-not-a-secret-postgres@127.0.0.1:${postgres_port}/hostlet"
  export BIND_ADDR="127.0.0.1:${api_port}"
  export PUBLIC_API_URL="http://127.0.0.1:${api_port}"
  export PUBLIC_WEB_URL="http://127.0.0.1:3000"
  export PUBLIC_WEBHOOK_URL="http://127.0.0.1:${api_port}"
  export HOSTLET_ALLOWED_WEB_ORIGINS="http://127.0.0.1:3000"
  export HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS=false
  export HOSTLET_SETUP_TOKEN=7b3f9c4a0e21d58f93a64b7c2d10e8f5
  export HOSTLET_ALLOWED_GITHUB_LOGINS=ci-user
  export ENCRYPTION_KEY=YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=
  export JOB_SIGNING_SECRET=9f6b2c8d4a1e73f05c92d6b4180aef35
  export SESSION_SECRET=2d7a91c0f4b63e8d5a20c7f149b6e3d8
  export LOCAL_AGENT_TOKEN=4d89f4e18a7bb4a01b51c83924492f46
  export GITHUB_WEBHOOK_SECRET=8c2f0b95d7a14e63b491f0d6a2c85e17
  export HOSTLET_UPDATE_CHECKS=false
}
