#!/usr/bin/env bash
# Negative test for check-line-cap.sh. Proves the regression guard FAILS on a
# *_partN.rs shard filename and on an include!("..._partN.rs") decomposition
# chain, locking in rewrite-guide criteria #1/#2 so they can't silently regress.
# Uses untracked fixtures (the guard scans --others), so it never touches the
# git index.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

shard_fixture="zz_linecap_selftest_part1.rs"
include_fixture="zz_linecap_selftest_include.rs"
cleanup() { rm -f "${shard_fixture}" "${include_fixture}"; }
trap cleanup EXIT

fail() { echo "check-line-cap self-test FAILED: $1" >&2; exit 1; }

# 1) a *_partN.rs shard filename must be rejected.
printf 'fn x() {}\n' > "${shard_fixture}"
if scripts/check-line-cap.sh >/dev/null 2>&1; then
  fail "guard accepted an (untracked) ${shard_fixture} shard file"
fi
rm -f "${shard_fixture}"

# 2) an include!("..._partN.rs") decomposition chain must be rejected.
printf 'include!("foo_part1.rs");\n' > "${include_fixture}"
if scripts/check-line-cap.sh >/dev/null 2>&1; then
  fail "guard accepted an include!(\"..._part1.rs\") decomposition chain"
fi
rm -f "${include_fixture}"

echo "check-line-cap self-test passed"
