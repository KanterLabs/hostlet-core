#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/scripts/ci-db-tests.sh"

run_case() {
  local expected="$1"
  local name="$2"
  local url="$3"
  shift 3
  local output
  local status

  set +e
  output="$(
    HOSTLET_DB_TEST_URL="${url}" \
      "$@" \
      bash "${SCRIPT}" --validate-only 2>&1
  )"
  status="$?"
  set -e

  if [ "${expected}" = "pass" ] && [ "${status}" -ne 0 ]; then
    echo "${name}: expected pass, got ${status}: ${output}" >&2
    exit 1
  fi
  if [ "${expected}" = "fail" ] && [ "${status}" -eq 0 ]; then
    echo "${name}: expected refusal, got pass" >&2
    exit 1
  fi
}

run_case pass loopback-ip postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test env
run_case pass loopback-name postgres://ci:password@localhost:5432/hostlet_ci_test env
run_case pass loopback-ipv6 postgresql://ci:password@[::1]:5432/hostlet_ci_test env
run_case pass safe-query-options \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?sslmode=disable&application_name=hostlet-ci&statement-cache-capacity=0' \
  env
run_case pass encoded-safe-query-key \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?ssl%6dode=disable' \
  env
run_case fail remote-host postgresql://ci:password@db.example.com:5432/hostlet_ci_test env
run_case fail substring-contest postgresql://ci:password@127.0.0.1:5432/contest env
run_case fail substring-testimonials postgresql://ci:password@127.0.0.1:5432/testimonials env
run_case fail wrong-scheme mysql://ci:password@127.0.0.1:5432/hostlet_ci_test env
run_case fail query-host \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?host=db.example.com' env
run_case fail query-hostaddr \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?hostaddr=203.0.113.10' env
run_case fail query-port \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?port=15432' env
run_case fail query-dbname \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?dbname=production' env
run_case fail query-host-percent-encoded \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?%68%6f%73%74=db.example.com' \
  env
run_case fail query-hostaddr-percent-encoded \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?host%61ddr=203.0.113.10' \
  env
run_case fail query-port-percent-encoded \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?p%6frt=15432' env
run_case fail query-dbname-percent-encoded \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?db%6eame=production' env
run_case fail query-host-case-variant \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?HOST=db.example.com' env
run_case fail query-duplicate-routing-key \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?sslmode=disable&host=127.0.0.1&host=db.example.com' \
  env
run_case fail query-libpq-options \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?options=-c%20statement_timeout%3D0' \
  env
run_case fail query-unknown \
  'postgresql://ci:password@127.0.0.1:5432/hostlet_ci_test?future_option=value' env
run_case pass explicit-override postgresql://ci:password@db.example.com:5432/disposable \
  env HOSTLET_ALLOW_DESTRUCTIVE_DB_TESTS=1
run_case fail explicit-override-still-rejects-routing-query \
  'postgresql://ci:password@db.example.com:5432/disposable?host=prod.example.com' \
  env HOSTLET_ALLOW_DESTRUCTIVE_DB_TESTS=1

set +e
missing_output="$(env -u HOSTLET_DB_TEST_URL bash "${SCRIPT}" --validate-only 2>&1)"
missing_status="$?"
set -e
if [ "${missing_status}" -eq 0 ]; then
  echo "missing-url: expected refusal, got pass: ${missing_output}" >&2
  exit 1
fi

echo "ci-db-tests self-test passed"
