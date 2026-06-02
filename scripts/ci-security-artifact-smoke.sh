#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(git -C "${SCRIPT_ROOT}" rev-parse --show-toplevel)"
cd "${ROOT}"

SCAN_OUT="$(mktemp "${TMPDIR:-/tmp}/hostlet-web-secret-scan.XXXXXX")"
trap 'rm -f "${SCAN_OUT}"' EXIT

# Markers that must never appear in committed/built artifacts: third-party secret
# prefixes (Stripe, GitHub tokens, webhook signing) plus Hostlet's own env-var
# secret names. Kept as a named array so adding a marker is a one-line edit and
# the regex passed to rg is assembled from it.
secret_markers=(
  "sk_live_" "sk_test_" "pk_live_" "pk_test_"
  "whsec_" "ghp_" "github_pat_"
  "CLOUDFLARE_API_TOKEN" "GITHUB_APP_PRIVATE_KEY"
  "HOSTLET_SESSION_SECRET" "HOSTLET_JOB_SIGNING_SECRET"
  "HOSTLET_ENCRYPTION_KEY" "LOCAL_AGENT_TOKEN"
)
secret_marker_re="$(IFS='|'; echo "${secret_markers[*]}")"

# fail <message> [detail-command...]: print the failure, optionally dump details
# to stderr, then exit non-zero. Unifies the copy-pasted echo+dump+exit shape.
fail() {
  local message="$1"
  shift
  echo "${message}" >&2
  if [ "$#" -gt 0 ]; then
    "$@" >&2 || true
  fi
  exit 1
}

for ignored in .env .env.prod infra/.env dist/ dist/hostlet-release.json; do
  git -C "${ROOT}" check-ignore -q -- "${ignored}" || {
    echo "${ignored} is not ignored" >&2
    exit 1
  }
done

if git -C "${ROOT}" ls-files .env .env.prod infra/.env dist 2>/dev/null | grep -q .; then
  fail "Secret files or release artifacts are tracked" \
    git -C "${ROOT}" ls-files .env .env.prod infra/.env dist
fi

if [ -d apps/web/.next ]; then
  if rg -n --hidden --no-ignore-vcs "${secret_marker_re}" apps/web/.next >"${SCAN_OUT}"; then
    fail "Potential secret marker found in web build output" cat "${SCAN_OUT}"
  fi
fi

if [ -d dist ]; then
  if find dist -type f \( -name '.env' -o -name '.env.prod' -o -name '*.pem' -o -name '*private*key*' \) | grep -q .; then
    fail "Release dist contains forbidden secret-like files" find dist -type f
  fi
fi

echo "security artifact smoke passed"
