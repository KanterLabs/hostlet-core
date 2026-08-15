#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-self-hosted-lib-test.XXXXXX")"
trap 'rm -rf "${TMP_DIR}"' EXIT

mkdir -p "${TMP_DIR}/bin"

cat >"${TMP_DIR}/bin/docker" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${HOSTLET_SELF_HOSTED_LIB_TEST_DOCKER_CALLS}"
exit 1
SH

cat >"${TMP_DIR}/bin/sleep" <<'SH'
#!/usr/bin/env bash
exit 0
SH

chmod +x "${TMP_DIR}/bin/docker" "${TMP_DIR}/bin/sleep"
: >"${TMP_DIR}/docker-calls"

set +e
PATH="${TMP_DIR}/bin:${PATH}" \
  HOSTLET_SELF_HOSTED_LIB_TEST_DOCKER_CALLS="${TMP_DIR}/docker-calls" \
  POSTGRES_CONTAINER=hostlet-ci-selftest-postgres \
  bash -c 'set -euo pipefail; source "$1"; wait_postgres_ready' \
  bash "${ROOT}/scripts/ci-self-hosted-lib.sh" \
  >"${TMP_DIR}/stdout" 2>"${TMP_DIR}/stderr"
status="$?"
set -e

if [ "${status}" -eq 0 ]; then
  echo "all-probes-fail: wait_postgres_ready unexpectedly succeeded" >&2
  exit 1
fi
if ! grep -Fqi 'timed out waiting for PostgreSQL readiness after 60 attempts' "${TMP_DIR}/stderr"; then
  echo "all-probes-fail: missing readiness timeout error" >&2
  cat "${TMP_DIR}/stderr" >&2
  exit 1
fi
if [ "$(wc -l <"${TMP_DIR}/docker-calls")" -ne 60 ]; then
  echo "all-probes-fail: expected 60 failed readiness probes" >&2
  cat "${TMP_DIR}/docker-calls" >&2
  exit 1
fi

run_readiness_caller_case() {
  local name="$1"
  local script="$2"
  local expected_error="$3"
  local case_dir="${TMP_DIR}/${name}"
  local sentinel_dir="${case_dir}/sentinels"
  local runner_temp="${case_dir}/runner-temp"
  local cargo_home="${case_dir}/cargo-home"
  local docker_config="${case_dir}/docker-config"
  local status exec_calls

  mkdir -p "${case_dir}/bin" "${sentinel_dir}" "${cargo_home}/bin" \
    "${docker_config}/cli-plugins"

  cat >"${case_dir}/bin/docker" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${HOSTLET_SELF_HOSTED_LIB_TEST_DOCKER_CALLS}"
case "${1:-}" in
  exec)
    exit 1
    ;;
  port)
    : >"${HOSTLET_SELF_HOSTED_LIB_TEST_SENTINELS}/postgres-port-discovery"
    printf '127.0.0.1:5432\n'
    exit 0
    ;;
  run)
    case " $* " in
      *' httpd:'*|*' zot-'*)
        : >"${HOSTLET_SELF_HOSTED_LIB_TEST_SENTINELS}/registry-startup"
        ;;
    esac
    exit 0
    ;;
  *)
    exit 0
    ;;
esac
SH

  cat >"${case_dir}/bin/sleep" <<'SH'
#!/usr/bin/env bash
exit 0
SH

  cat >"${cargo_home}/bin/cargo" <<'SH'
#!/usr/bin/env bash
: >"${HOSTLET_SELF_HOSTED_LIB_TEST_SENTINELS}/cargo-build"
exit 0
SH

  cat >"${case_dir}/bin/railpack" <<'SH'
#!/usr/bin/env bash
: >"${HOSTLET_SELF_HOSTED_LIB_TEST_SENTINELS}/railpack-startup"
exit 0
SH

  for plugin in docker-buildx docker-compose; do
    cat >"${docker_config}/cli-plugins/${plugin}" <<'SH'
#!/usr/bin/env bash
exit 0
SH
  done
  chmod +x "${case_dir}/bin/docker" "${case_dir}/bin/sleep" \
    "${cargo_home}/bin/cargo" "${case_dir}/bin/railpack" \
    "${docker_config}/cli-plugins/docker-buildx" \
    "${docker_config}/cli-plugins/docker-compose"
  : >"${case_dir}/docker-calls"

  set +e
  env \
    PATH="${case_dir}/bin:${PATH}" \
    HOME="${case_dir}/home" \
    CARGO_HOME="${cargo_home}" \
    DOCKER_CONFIG="${docker_config}" \
    RUNNER_TEMP="${runner_temp}" \
    GITHUB_RUN_ID="core-17-selftest-${name}" \
    HOSTLET_RAILPACK_BIN="${case_dir}/bin/railpack" \
    HOSTLET_SELF_HOSTED_LIB_TEST_DOCKER_CALLS="${case_dir}/docker-calls" \
    HOSTLET_SELF_HOSTED_LIB_TEST_SENTINELS="${sentinel_dir}" \
    bash "${ROOT}/scripts/${script}" \
    >"${case_dir}/stdout" 2>"${case_dir}/stderr"
  status="$?"
  set -e

  if [ "${status}" -eq 0 ]; then
    echo "${name}: readiness failure unexpectedly succeeded" >&2
    cat "${case_dir}/stdout" >&2
    cat "${case_dir}/stderr" >&2
    exit 1
  fi
  if ! grep -Fq -- "${expected_error}" "${case_dir}/stderr"; then
    echo "${name}: missing caller readiness error" >&2
    cat "${case_dir}/stderr" >&2
    exit 1
  fi
  if ! grep -Fq -- 'Timed out waiting for PostgreSQL readiness after 60 attempts' "${case_dir}/stderr"; then
    echo "${name}: missing helper readiness timeout error" >&2
    cat "${case_dir}/stderr" >&2
    exit 1
  fi
  exec_calls="$(grep -c '^exec ' "${case_dir}/docker-calls" || true)"
  if [ "${exec_calls}" -lt 60 ]; then
    echo "${name}: expected at least 60 failed readiness probes, got ${exec_calls}" >&2
    cat "${case_dir}/docker-calls" >&2
    exit 1
  fi
  for sentinel in postgres-port-discovery cargo-build registry-startup railpack-startup; do
    if [ -e "${sentinel_dir}/${sentinel}" ]; then
      echo "${name}: reached startup sentinel ${sentinel}" >&2
      exit 1
    fi
  done
}

run_readiness_caller_case \
  api-smoke \
  ci-self-hosted-api-smoke.sh \
  'PostgreSQL readiness failed; aborting self-hosted API smoke before API startup'
run_readiness_caller_case \
  deploy-e2e \
  ci-self-hosted-deploy-e2e.sh \
  'PostgreSQL readiness failed; aborting self-hosted deploy E2E before registry startup'
run_readiness_caller_case \
  remote-build-e2e \
  ci-remote-build-e2e.sh \
  'PostgreSQL readiness failed; aborting remote-build E2E before registry startup'

echo "ci-self-hosted-lib readiness and caller self-test passed"
