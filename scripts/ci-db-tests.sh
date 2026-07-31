#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_DB_TESTS=55

if [ -z "${HOSTLET_DB_TEST_URL:-}" ]; then
  echo "HOSTLET_DB_TEST_URL is required" >&2
  exit 1
fi

# Parse without echoing the credential-bearing URL. These tests intentionally
# truncate shared application tables, so the safe default is deliberately
# narrow: only the exact CI database on a loopback host.
database_target="$(
  python3 - "${HOSTLET_DB_TEST_URL}" "${HOSTLET_ALLOW_DESTRUCTIVE_DB_TESTS:-0}" <<'PY'
import sys
from urllib.parse import parse_qsl, unquote, urlparse

try:
    parsed = urlparse(sys.argv[1])
    database = unquote(parsed.path.removeprefix("/"), errors="strict")
    query_pairs = parse_qsl(
        parsed.query,
        keep_blank_values=True,
        strict_parsing=True,
        encoding="utf-8",
        errors="strict",
        max_num_fields=16,
    )
except (TypeError, UnicodeError, ValueError):
    raise SystemExit("HOSTLET_DB_TEST_URL is not a valid PostgreSQL URL")

if parsed.scheme not in {"postgres", "postgresql"} or not parsed.hostname or not database:
    raise SystemExit("HOSTLET_DB_TEST_URL is not a valid PostgreSQL URL")

# SQLx applies these query parameters after the URL authority/path and accepts
# percent-decoded duplicate keys. Never allow a visually safe URL to redirect
# destructive tests to another server, port, or database.
target_routing_keys = {"host", "hostaddr", "port", "dbname"}
safe_query_keys = {
    "application_name",
    "ssl-mode",
    "sslmode",
    "statement-cache-capacity",
}
for key, _ in query_pairs:
    if key in target_routing_keys:
        raise SystemExit(
            "HOSTLET_DB_TEST_URL must not contain target-routing query parameters"
        )
    if key not in safe_query_keys:
        raise SystemExit(
            "HOSTLET_DB_TEST_URL contains an unsupported query parameter"
        )

if parsed.fragment:
    raise SystemExit("HOSTLET_DB_TEST_URL must not contain a fragment")
if any(ord(char) < 32 or ord(char) == 127 for char in parsed.hostname + database):
    raise SystemExit("HOSTLET_DB_TEST_URL contains control characters")

allow_destructive = sys.argv[2] == "1"
safe_hosts = {"localhost", "127.0.0.1", "::1"}
if not allow_destructive and (
    parsed.hostname not in safe_hosts or database != "hostlet_ci_test"
):
    raise SystemExit(
        "refusing destructive DB tests: expected loopback database 'hostlet_ci_test'\n"
        "set HOSTLET_ALLOW_DESTRUCTIVE_DB_TESTS=1 only for an intentionally disposable target"
    )

print(database)
PY
)"
database_name="${database_target}"

export HOSTLET_DB_TEST_REQUIRED=1
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-never}"
export PGCONNECT_TIMEOUT="${PGCONNECT_TIMEOUT:-5}"
export PGOPTIONS="${PGOPTIONS:+${PGOPTIONS} }-c statement_timeout=15000 -c lock_timeout=5000"
# Deploy retries now enqueue the universal build phase. The DB suite only
# verifies the queued records, so use non-secret loopback transport fixtures;
# no registry process or network request is needed.
export HOSTLET_ARTIFACT_REGISTRY_LOCAL_URL="${HOSTLET_ARTIFACT_REGISTRY_LOCAL_URL:-http://127.0.0.1:5000}"
export HOSTLET_ARTIFACT_REGISTRY_PUSH_USERNAME="${HOSTLET_ARTIFACT_REGISTRY_PUSH_USERNAME:-ci-builder}"
export HOSTLET_ARTIFACT_REGISTRY_PUSH_PASSWORD="${HOSTLET_ARTIFACT_REGISTRY_PUSH_PASSWORD:-test-only-builder-credential}"
export HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME="${HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME:-ci-runner}"
export HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD="${HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD:-test-only-runner-credential}"

cd "${ROOT}"

if [ "${1:-}" = "--validate-only" ]; then
  echo "validated disposable database target '${database_name}'"
  exit 0
fi
if [ "$#" -ne 0 ]; then
  echo "usage: $0 [--validate-only]" >&2
  exit 2
fi

test_list="$(cargo test --package hostlet-api --lib 'db_' -- --list)"
discovered_count="$(
  awk '/: test$/ { count += 1 } END { print count + 0 }' <<< "${test_list}"
)"

if [ "${discovered_count}" -ne "${EXPECTED_DB_TESTS}" ]; then
  echo "DB test discovery found ${discovered_count}; expected exactly ${EXPECTED_DB_TESTS}" >&2
  echo "Review every added, removed, or renamed db_ test and update the explicit inventory." >&2
  exit 1
fi

echo "running database connection/query preflight"
cargo test --package hostlet-api --lib \
  state::tests::required_database_preflight -- \
  --exact --test-threads=1

echo "running ${discovered_count} DB-backed tests serially against disposable database '${database_name}'"
cargo test --package hostlet-api --lib 'db_' -- --test-threads=1
