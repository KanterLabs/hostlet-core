#!/usr/bin/env bash
set -euo pipefail

# Prepare the exact Core release PR without waiting for checks or merging it.
# The branch is always release-candidate/vX.Y.Z and always points at the
# currently fetched origin/staging commit. Existing branches/PRs are reused
# only when their head and base identities are exact matches.

usage() {
  cat >&2 <<'EOF'
usage: scripts/prepare-release-pr.sh [--dry-run] [--repo OWNER/REPO] VERSION

Environment overrides are available for adapters/tests:
  HOSTLET_RELEASE_REMOTE (default: origin)
  HOSTLET_RELEASE_SOURCE_BRANCH (default: staging)
  HOSTLET_RELEASE_BASE_BRANCH (default: main)
  HOSTLET_RELEASE_REPO (default: GITHUB_REPOSITORY)
  GH_BIN (default: gh), GIT_BIN (default: git)
EOF
}

version=""
remote="${HOSTLET_RELEASE_REMOTE:-origin}"
source_branch="${HOSTLET_RELEASE_SOURCE_BRANCH:-staging}"
base_branch="${HOSTLET_RELEASE_BASE_BRANCH:-main}"
repo="${HOSTLET_RELEASE_REPO:-${GITHUB_REPOSITORY:-}}"
gh_bin="${GH_BIN:-gh}"
git_bin="${GIT_BIN:-git}"
dry_run=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      shift
      ;;
    --repo)
      [ "$#" -ge 2 ] || { usage; exit 2; }
      repo="$2"
      shift 2
      ;;
    -h|--help)
      usage >&2
      exit 0
      ;;
    --)
      shift
      [ "$#" -eq 1 ] || { usage; exit 2; }
      version="$1"
      shift
      ;;
    -*)
      echo "unknown option: $1" >&2
      usage
      exit 2
      ;;
    *)
      [ -z "$version" ] || { echo "version specified more than once" >&2; exit 2; }
      version="$1"
      shift
      ;;
  esac
done

[ -n "$version" ] || { usage; exit 2; }
version="${version#v}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version must be stable X.Y.Z: $version" >&2
  exit 2
fi
branch="release-candidate/v${version}"

if ! root="$($git_bin rev-parse --show-toplevel 2>/dev/null)"; then
  echo "must run inside a git worktree" >&2
  exit 1
fi
cd "$root"

# Keep the exact remote branch identities available locally. The only write
# below is the explicitly requested release branch push.
$git_bin fetch --quiet "$remote" \
  "refs/heads/${source_branch}:refs/remotes/${remote}/${source_branch}" \
  "refs/heads/${base_branch}:refs/remotes/${remote}/${base_branch}"
source_sha="$($git_bin rev-parse --verify "refs/remotes/${remote}/${source_branch}^{commit}")"
base_sha="$($git_bin rev-parse --verify "refs/remotes/${remote}/${base_branch}^{commit}")"

remote_branch_sha="$($git_bin ls-remote --heads "$remote" "refs/heads/${branch}" | awk 'NR == 1 { print $1 }')"
if [ -n "$remote_branch_sha" ]; then
  if [ "$remote_branch_sha" != "$source_sha" ]; then
    echo "existing ${branch} points to ${remote_branch_sha}, expected staging ${source_sha}" >&2
    exit 1
  fi
elif [ "$dry_run" -eq 0 ]; then
  $git_bin push --quiet "$remote" "${source_sha}:refs/heads/${branch}"
else
  remote_branch_sha="$source_sha"
fi

if [ "$dry_run" -eq 1 ]; then
  number="${HOSTLET_RELEASE_PR_DRY_RUN_NUMBER:-0}"
  python3 - "$number" "$branch" "$source_sha" "$base_branch" "$base_sha" <<'PY'
import json
import sys

print(json.dumps({
    "number": int(sys.argv[1]),
    "head": sys.argv[2],
    "headSha": sys.argv[3],
    "base": sys.argv[4],
    "baseSha": sys.argv[5],
}, sort_keys=True, separators=(",", ":")))
PY
  exit 0
fi

[ -n "$repo" ] || {
  echo "repository is required (set GITHUB_REPOSITORY or --repo)" >&2
  exit 2
}
if [[ ! "$repo" =~ ^[^/]+/[^/]+$ ]]; then
  echo "repository must be OWNER/REPO: $repo" >&2
  exit 2
fi

# Query every PR for this head branch (not only base=main), so a stale PR to a
# different base is rejected instead of silently creating a second collision.
pulls_json="$($gh_bin api "repos/${repo}/pulls?state=all&head=${repo%/*}:${branch}&per_page=100")"
summary="$(LIST_JSON="$pulls_json" python3 - "$branch" "$source_sha" "$base_branch" "$base_sha" <<'PY'
import json
import os
import sys

rows = json.loads(os.environ["LIST_JSON"])
if not isinstance(rows, list):
    raise SystemExit("GitHub pull-request list was not an array")
if len(rows) > 1:
    raise SystemExit("multiple pull requests already use the release-candidate branch")
if not rows:
    print(json.dumps({"exists": False}, separators=(",", ":")))
    raise SystemExit(0)
row = rows[0]
head = row.get("head") or {}
base = row.get("base") or {}
expected = {
    "head.ref": sys.argv[1],
    "head.sha": sys.argv[2],
    "base.ref": sys.argv[3],
    "base.sha": sys.argv[4],
}
actual = {
    "head.ref": head.get("ref"),
    "head.sha": head.get("sha"),
    "base.ref": base.get("ref"),
    "base.sha": base.get("sha"),
}
if row.get("state") != "open":
    raise SystemExit("the release-candidate pull request exists but is not open")
for key, value in expected.items():
    if actual[key] != value:
        raise SystemExit(f"pull request {key} is {actual[key]!r}, expected {value!r}")
number = row.get("number")
if isinstance(number, bool) or not isinstance(number, int) or number < 1:
    raise SystemExit("existing pull request has no positive number")
print(json.dumps({
    "exists": True,
    "number": number,
    "head": actual["head.ref"],
    "headSha": actual["head.sha"],
    "base": actual["base.ref"],
    "baseSha": actual["base.sha"],
}, sort_keys=True, separators=(",", ":")))
PY
)"

if [ "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["exists"])' "$summary")" = True ]; then
  printf '%s\n' "$summary" | python3 -c 'import json,sys; value=json.load(sys.stdin); value.pop("exists",None); print(json.dumps(value, sort_keys=True, separators=(",", ":")))'
  exit 0
fi

created_json="$($gh_bin api --method POST "repos/${repo}/pulls" \
  --raw-field "title=Release candidate v${version}" \
  --raw-field "head=${branch}" \
  --raw-field "base=${base_branch}" \
  --raw-field "body=Prepared from the exact origin/${source_branch} commit ${source_sha}. Do not merge until the release candidate gate is green.")"
created_summary="$(CREATED_JSON="$created_json" python3 - "$branch" "$source_sha" "$base_branch" "$base_sha" <<'PY'
import json
import os
import sys

row = json.loads(os.environ["CREATED_JSON"])
head = row.get("head") or {}
base = row.get("base") or {}
expected = (sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])
actual = (head.get("ref"), head.get("sha"), base.get("ref"), base.get("sha"))
if actual != expected:
    raise SystemExit(f"created pull request identity {actual!r} does not match {expected!r}")
number = row.get("number")
if isinstance(number, bool) or not isinstance(number, int) or number < 1:
    raise SystemExit("created pull request has no positive number")
print(json.dumps({
    "number": number,
    "head": actual[0],
    "headSha": actual[1],
    "base": actual[2],
    "baseSha": actual[3],
}, sort_keys=True, separators=(",", ":")))
PY
)"
printf '%s\n' "$created_summary"
