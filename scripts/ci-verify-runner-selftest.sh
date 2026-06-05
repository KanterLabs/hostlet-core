#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/scripts/ci-verify-runner.sh"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-runner-verify-test.XXXXXX")"
trap 'rm -rf "${TMP_DIR}"' EXIT

mkdir -p "${TMP_DIR}/bin"

cat >"${TMP_DIR}/bin/df" <<'SH'
#!/usr/bin/env bash
path="${2:-${1:-/}}"
case "${path}" in
  /) percent="${STUB_ROOT_DF_PERCENT:-12}" ;;
  /var/lib/docker) percent="${STUB_DOCKER_DF_PERCENT:-12}" ;;
  *) percent=12 ;;
esac
printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\n'
printf '/dev/test 100 10 90 %s%% %s\n' "${percent}" "${path}"
SH

cat >"${TMP_DIR}/bin/docker" <<'SH'
#!/usr/bin/env bash
if [ "${STUB_DOCKER_FAIL:-0}" = "1" ]; then
  exit 1
fi
case "${1:-}" in
  info) exit 0 ;;
  *) exit 0 ;;
esac
SH

cat >"${TMP_DIR}/bin/mountpoint" <<'SH'
#!/usr/bin/env bash
if [ "${STUB_DOCKER_NOT_MOUNTED:-0}" = "1" ]; then
  exit 1
fi
exit 0
SH

chmod +x "${TMP_DIR}/bin/df" "${TMP_DIR}/bin/docker" "${TMP_DIR}/bin/mountpoint"

run_case() {
  local expected="$1"
  local name="$2"
  shift 2
  set +e
  env \
    PATH="${TMP_DIR}/bin:${PATH}" \
    RUNNER_NAME=hostlet-core-homelab-2 \
    RUNNER_OS=Linux \
    RUNNER_ARCH=X64 \
    "$@" \
    "${SCRIPT}" >"${TMP_DIR}/${name}.out" 2>"${TMP_DIR}/${name}.err"
  local status="$?"
  set -e
  if [ "${expected}" = "pass" ] && [ "${status}" -ne 0 ]; then
    echo "${name}: expected pass, got ${status}" >&2
    cat "${TMP_DIR}/${name}.err" >&2
    exit 1
  fi
  if [ "${expected}" = "fail" ] && [ "${status}" -eq 0 ]; then
    echo "${name}: expected fail, got pass" >&2
    cat "${TMP_DIR}/${name}.out" >&2
    exit 1
  fi
}

run_case pass success
run_case fail missing-env env -u RUNNER_NAME
run_case fail wrong-name HOSTLET_ALLOWED_RUNNER_NAMES=hostlet-core-homelab-3
run_case fail wrong-os RUNNER_OS=macOS
run_case fail wrong-arch RUNNER_ARCH=ARM64
run_case fail low-root-disk STUB_ROOT_DF_PERCENT=95
run_case fail docker-unavailable STUB_DOCKER_FAIL=1
run_case fail docker-not-mounted STUB_DOCKER_NOT_MOUNTED=1
run_case fail low-docker-disk STUB_DOCKER_DF_PERCENT=95
run_case pass root-disk-override STUB_ROOT_DF_PERCENT=95 HOSTLET_ALLOW_LOW_DISK=1
run_case pass docker-disk-override STUB_DOCKER_DF_PERCENT=95 HOSTLET_ALLOW_LOW_DOCKER_DISK=1

echo "ci-verify-runner self-test passed"
