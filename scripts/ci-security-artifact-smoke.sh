#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

for ignored in .env .env.prod infra/.env dist dist/hostlet-release.json; do
  git check-ignore -q "${ignored}" || {
    echo "${ignored} is not ignored" >&2
    exit 1
  }
done

if git ls-files .env .env.prod infra/.env dist 2>/dev/null | grep -q .; then
  echo "Secret files or release artifacts are tracked" >&2
  git ls-files .env .env.prod infra/.env dist >&2
  exit 1
fi

if [ -d apps/web/.next ]; then
  if rg -n --hidden --no-ignore-vcs \
    "sk_live_|sk_test_|pk_live_|pk_test_|whsec_|ghp_|github_pat_|CLOUDFLARE_API_TOKEN|GITHUB_APP_PRIVATE_KEY|HOSTLET_SESSION_SECRET|HOSTLET_JOB_SIGNING_SECRET|HOSTLET_ENCRYPTION_KEY|LOCAL_AGENT_TOKEN" \
    apps/web/.next >/tmp/hostlet-web-secret-scan.txt; then
    echo "Potential secret marker found in web build output" >&2
    cat /tmp/hostlet-web-secret-scan.txt >&2
    exit 1
  fi
fi

if [ -d dist ]; then
  if find dist -type f \( -name '.env' -o -name '.env.prod' -o -name '*.pem' -o -name '*private*key*' \) | grep -q .; then
    echo "Release dist contains forbidden secret-like files" >&2
    find dist -type f >&2
    exit 1
  fi
fi

echo "security artifact smoke passed"
