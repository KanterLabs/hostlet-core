#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/scripts/prepare-release-pr.sh"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-prepare-release-pr.XXXXXX")"
trap 'rm -rf -- "${TMP_ROOT}"' EXIT

ORIGIN="${TMP_ROOT}/origin.git"
WORK="${TMP_ROOT}/work"
GH_STUB="${TMP_ROOT}/gh"

git init --quiet --bare "${ORIGIN}"
git init --quiet --initial-branch=main "${WORK}"
git -C "${WORK}" config user.name "Hostlet Release Selftest"
git -C "${WORK}" config user.email "release-selftest@hostlet.invalid"
git -C "${WORK}" remote add origin "${ORIGIN}"
mkdir -p "${WORK}/apps/cli" "${WORK}/apps/api" "${WORK}/apps/agent"
for manifest in apps/cli/Cargo.toml apps/api/Cargo.toml apps/agent/Cargo.toml; do
  printf 'version = "0.2.25"\n' > "${WORK}/${manifest}"
done
git -C "${WORK}" add apps
git -C "${WORK}" commit --quiet --allow-empty -m "main base"
git -C "${WORK}" push --quiet --set-upstream origin main
git -C "${WORK}" switch --quiet --create staging
git -C "${WORK}" commit --quiet --allow-empty -m "staging candidate"
git -C "${WORK}" push --quiet --set-upstream origin staging
SOURCE_SHA="$(git -C "${WORK}" rev-parse HEAD)"
BASE_SHA="$(git -C "${WORK}" rev-parse origin/main)"

cat > "${GH_STUB}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "${1:-}" = api ] || { echo "stub only supports gh api" >&2; exit 2; }
shift
method=GET
while [ "$#" -gt 0 ]; do
  case "$1" in
    --method) method="$2"; shift 2 ;;
    --raw-field) shift 2 ;;
    repos/*) shift ;;
    *) shift ;;
  esac
done
if [ "${method}" = GET ]; then
  if [ "${GH_STUB_STATE:-none}" = existing ]; then
    printf '[{"number":42,"state":"open","head":{"ref":"release-candidate/v0.2.25","sha":"%s"},"base":{"ref":"main","sha":"%s"}}]\n' "${GH_STUB_SOURCE}" "${GH_STUB_BASE}"
  else
    printf '[]\n'
  fi
else
  printf '{"number":42,"state":"open","head":{"ref":"release-candidate/v0.2.25","sha":"%s"},"base":{"ref":"main","sha":"%s"}}\n' "${GH_STUB_SOURCE}" "${GH_STUB_BASE}"
fi
EOF
chmod +x "${GH_STUB}"

run_prepare() {
  GH_BIN="${GH_STUB}" \
  GH_STUB_SOURCE="${SOURCE_SHA}" \
  GH_STUB_BASE="${BASE_SHA}" \
  GITHUB_REPOSITORY="example/hostlet-core" \
    bash "${SCRIPT}" --repo example/hostlet-core 0.2.25
}

created="$(cd "${WORK}" && GH_STUB_STATE=none run_prepare)"
python3 - "${created}" "${SOURCE_SHA}" "${BASE_SHA}" <<'PY'
import json
import sys

value = json.loads(sys.argv[1])
assert value == {
    "number": 42,
    "head": "release-candidate/v0.2.25",
    "headSha": sys.argv[2],
    "base": "main",
    "baseSha": sys.argv[3],
}
PY

reused="$(cd "${WORK}" && GH_STUB_STATE=existing run_prepare)"
[ "${reused}" = "${created}" ]

# A changed base must reject reuse rather than silently retargeting the PR.
git -C "${WORK}" switch --quiet main
git -C "${WORK}" commit --quiet --allow-empty -m "main moved"
git -C "${WORK}" push --quiet origin main
if (cd "${WORK}" && GH_STUB_STATE=existing run_prepare >/dev/null 2>&1); then
  echo "base mismatch was accepted" >&2
  exit 1
fi

# Dry-run is deterministic and does not require gh or push a branch.
dry="$(cd "${WORK}" && HOSTLET_RELEASE_PR_DRY_RUN_NUMBER=7 \
  bash "${SCRIPT}" --dry-run 0.2.25)"
python3 - "${dry}" <<'PY'
import json
import sys

value = json.loads(sys.argv[1])
assert value["number"] == 7
assert value["head"] == "release-candidate/v0.2.25"
assert value["base"] == "main"
assert len(value["headSha"]) == 40
assert len(value["baseSha"]) == 40
PY

# A pre-existing release tag is rejected before any branch/PR write.
git -C "${WORK}" tag v0.2.25 origin/main
git -C "${WORK}" push --quiet origin refs/tags/v0.2.25
if (cd "${WORK}" && HOSTLET_RELEASE_PR_DRY_RUN_NUMBER=7 \
    bash "${SCRIPT}" --dry-run 0.2.25 >/dev/null 2>&1); then
  echo "tag collision was accepted" >&2
  exit 1
fi

# A version mismatch is rejected before any branch/PR write.
git -C "${WORK}" switch --quiet --create staging-bad staging
printf 'version = "0.2.26"\n' > "${WORK}/apps/cli/Cargo.toml"
git -C "${WORK}" add apps/cli/Cargo.toml
git -C "${WORK}" commit --quiet -m "bad package version"
git -C "${WORK}" push --quiet --set-upstream origin staging-bad
if (cd "${WORK}" && HOSTLET_RELEASE_SOURCE_BRANCH=staging-bad \
    bash "${SCRIPT}" --dry-run 0.2.25 >/dev/null 2>&1); then
  echo "version mismatch was accepted" >&2
  exit 1
fi

# A branch collision with a different head is rejected before any PR API call.
git -C "${WORK}" switch --quiet staging
git -C "${WORK}" commit --quiet --allow-empty -m "collision head"
git -C "${WORK}" push --quiet origin \
  "HEAD:refs/heads/release-candidate/v0.2.25"
if (cd "${WORK}" && GH_BIN="${GH_STUB}" GITHUB_REPOSITORY=example/hostlet-core \
    bash "${SCRIPT}" --repo example/hostlet-core 0.2.25 >/dev/null 2>&1); then
  echo "branch collision was accepted" >&2
  exit 1
fi

echo "prepare-release-pr self-test passed"
