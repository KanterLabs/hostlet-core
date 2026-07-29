#!/usr/bin/env bash
set -euo pipefail

event_name="${GITHUB_EVENT_NAME:-}"
github_ref="${GITHUB_REF:-}"

if [ "${event_name}" = "workflow_dispatch" ]; then
  if [ "${github_ref}" != "refs/heads/main" ]; then
    echo "release dry runs are allowed only from refs/heads/main" >&2
    exit 1
  fi
  echo "release dry run is on refs/heads/main"
  exit 0
fi

if [ "${event_name}" != "push" ] || [[ "${github_ref}" != refs/tags/* ]]; then
  echo "release publication requires a stable tag push" >&2
  exit 1
fi

release_tag="${github_ref#refs/tags/}"
if [[ ! "${release_tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "release publication requires an exact stable tag vX.Y.Z" >&2
  exit 1
fi

github_sha="${GITHUB_SHA:-}"
if [[ ! "${github_sha}" =~ ^[0-9a-fA-F]{40,64}$ ]]; then
  echo "GITHUB_SHA must identify the tagged commit" >&2
  exit 1
fi

# Fetch the release branch explicitly so the decision is based on the current
# remote mainline, not on a potentially stale local tracking ref.
git fetch --quiet --no-tags --prune origin \
  '+refs/heads/main:refs/remotes/origin/main'

if ! candidate_commit="$(git rev-parse --verify "${github_sha}^{commit}")"; then
  echo "release tag target is not a commit: ${github_sha}" >&2
  exit 1
fi
if ! git rev-parse --verify 'refs/remotes/origin/main^{commit}' >/dev/null; then
  echo "could not resolve origin/main" >&2
  exit 1
fi

if ! git merge-base --is-ancestor "${candidate_commit}" refs/remotes/origin/main; then
  echo "refusing release: tag target ${candidate_commit} is not reachable from origin/main" >&2
  exit 1
fi

echo "release tag target ${candidate_commit} is reachable from origin/main"
