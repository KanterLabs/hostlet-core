#!/usr/bin/env python3
"""Validate Core release evidence for PR gates and main-push reuse.

The pull-request mode is fail-closed.  It only accepts an open, same-repository
release-candidate PR whose exact head has a fresh artifact from a successful
staging run.  Main-push mode is deliberately fail-open: an invalid or missing
receipt leaves the ordinary heavy CI lanes enabled, while a fully validated
merged release candidate allows those duplicate lanes to be skipped.
"""

from __future__ import annotations

import base64
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import zipfile
from collections.abc import Callable, Mapping
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib import error, parse, request


ARTIFACT_NAME = "core-release-candidate"
MAX_AGE_SECONDS = 14400
RELEASE_BRANCH_PATTERN = re.compile(r"^release-candidate/v(?P<version>[0-9]+\.[0-9]+\.[0-9]+)$")
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY_PATTERN = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
DEPENDENCIES = (
    "secrets",
    "rust",
    "web",
    "compose",
    "docker",
    "remote-build",
)
REVOCATION_DEPENDENCY = "clear-approval"


class EvidenceFailure(RuntimeError):
    """Expected fail-closed evidence failure."""


def required(environment: Mapping[str, str], name: str) -> str:
    value = environment.get(name, "")
    if not value:
        raise EvidenceFailure(f"required environment value {name} is missing")
    return value


def require_sha(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA_PATTERN.fullmatch(value):
        raise EvidenceFailure(f"{label} is not a full lowercase commit SHA")
    return value


def nested(value: Any, *keys: str) -> Any:
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def parse_time(value: Any, label: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise EvidenceFailure(f"{label} is missing")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise EvidenceFailure(f"{label} is malformed") from exc
    if parsed.tzinfo is None:
        raise EvidenceFailure(f"{label} is not timezone-aware")
    return parsed.astimezone(timezone.utc)


def require_run_id(value: Any, label: str) -> int:
    if isinstance(value, bool):
        raise EvidenceFailure(f"{label} is not a positive run ID")
    try:
        parsed = int(value)
    except (TypeError, ValueError) as exc:
        raise EvidenceFailure(f"{label} is not a positive run ID") from exc
    if parsed < 1:
        raise EvidenceFailure(f"{label} is not a positive run ID")
    return parsed


def validate_dependencies(payload: Any) -> None:
    """Require every ordinary approved-PR lane to have run successfully."""

    if not isinstance(payload, dict):
        raise EvidenceFailure("dependency results are not a JSON object")
    unexpected = sorted(set(payload) - {*DEPENDENCIES, REVOCATION_DEPENDENCY})
    if unexpected:
        raise EvidenceFailure("dependency results contain unexpected jobs: " + ", ".join(unexpected))
    failures = []
    for name in DEPENDENCIES:
        record = payload.get(name)
        result = record.get("result") if isinstance(record, dict) else None
        if result != "success":
            failures.append(f"{name}={result or 'missing'}")
    if failures:
        raise EvidenceFailure("ordinary PR dependencies did not succeed: " + ", ".join(failures))


def release_candidate_version(branch: Any) -> str | None:
    if not isinstance(branch, str):
        return None
    match = RELEASE_BRANCH_PATTERN.fullmatch(branch)
    return match.group("version") if match else None


def validate_live_pull(
    pull: Any,
    *,
    repository: str,
    expected_base_ref: str,
    expected_base_sha: str,
    expected_head_ref: str,
    expected_head_sha: str,
) -> str | None:
    """Return release version for an exact live PR, or None for ordinary PRs."""

    if not isinstance(pull, dict):
        raise EvidenceFailure("live pull-request response is not a JSON object")
    if pull.get("state") != "open":
        raise EvidenceFailure("pull request is not open")
    base_ref = nested(pull, "base", "ref")
    base_sha = require_sha(nested(pull, "base", "sha"), "live pull-request base")
    head_ref = nested(pull, "head", "ref")
    head_sha = require_sha(nested(pull, "head", "sha"), "live pull-request head")
    if base_ref != expected_base_ref or base_sha != expected_base_sha:
        raise EvidenceFailure("pull request base changed during validation")
    if head_ref != expected_head_ref or head_sha != expected_head_sha:
        raise EvidenceFailure("pull request head changed during validation")
    if nested(pull, "head", "repo", "full_name") != repository:
        raise EvidenceFailure("pull request head is not in the Core repository")
    return release_candidate_version(head_ref)


def is_merged_release_pull(pull: Any, *, repository: str, push_sha: str) -> bool:
    """Bind a main push to one exact merged release-candidate PR."""

    if not isinstance(pull, dict) or pull.get("merged_at") is None:
        return False
    if pull.get("merge_commit_sha") != push_sha:
        return False
    base = pull.get("base")
    head = pull.get("head")
    return (
        isinstance(base, dict)
        and base.get("ref") == "main"
        and nested(base, "repo", "full_name") == repository
        and isinstance(head, dict)
        and nested(head, "repo", "full_name") == repository
        and release_candidate_version(head.get("ref")) is not None
        and isinstance(head.get("sha"), str)
        and SHA_PATTERN.fullmatch(head["sha"]) is not None
    )


def _json_request(
    environment: Mapping[str, str],
    path: str,
    *,
    accept: str = "application/vnd.github+json",
) -> Any:
    token = required(environment, "GH_TOKEN")
    api_url = required(environment, "GITHUB_API_URL").rstrip("/")
    url = f"{api_url}/{path.lstrip('/')}"
    api_request = request.Request(
        url,
        headers={
            "Accept": accept,
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with request.urlopen(api_request, timeout=30) as response:
            if response.status != 200:
                raise EvidenceFailure(f"GitHub API returned HTTP {response.status}")
            payload = response.read()
    except EvidenceFailure:
        raise
    except (error.HTTPError, error.URLError, TimeoutError, OSError) as exc:
        raise EvidenceFailure(f"GitHub API request failed: {type(exc).__name__}") from exc
    try:
        return json.loads(payload)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise EvidenceFailure("GitHub API returned malformed JSON") from exc


def _bytes_request(environment: Mapping[str, str], path: str) -> bytes:
    token = required(environment, "GH_TOKEN")
    api_url = required(environment, "GITHUB_API_URL").rstrip("/")
    api_request = request.Request(
        f"{api_url}/{path.lstrip('/')}",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with request.urlopen(api_request, timeout=60) as response:
            if response.status != 200:
                raise EvidenceFailure(f"artifact download returned HTTP {response.status}")
            return response.read()
    except EvidenceFailure:
        raise
    except (error.HTTPError, error.URLError, TimeoutError, OSError) as exc:
        raise EvidenceFailure(f"artifact download failed: {type(exc).__name__}") from exc


def _repository(environment: Mapping[str, str]) -> str:
    repository = required(environment, "GITHUB_REPOSITORY")
    if not REPOSITORY_PATTERN.fullmatch(repository):
        raise EvidenceFailure("GITHUB_REPOSITORY is malformed")
    return repository


def fetch_live_pull(environment: Mapping[str, str]) -> Any:
    repository = _repository(environment)
    number = required(environment, "PR_NUMBER")
    if not number.isdigit() or int(number) < 1:
        raise EvidenceFailure("PR_NUMBER is malformed")
    return _json_request(environment, f"repos/{repository}/pulls/{int(number)}")


def fetch_commit_pulls(environment: Mapping[str, str], push_sha: str) -> list[Any]:
    repository = _repository(environment)
    payload = _json_request(environment, f"repos/{repository}/commits/{push_sha}/pulls")
    if not isinstance(payload, list):
        raise EvidenceFailure("commit pull-request response is not an array")
    return payload


def _artifact_time(artifact: Mapping[str, Any], now: datetime) -> datetime:
    expires_at = parse_time(artifact.get("expires_at"), "artifact expires_at")
    if artifact.get("expired") is True or expires_at <= now:
        raise EvidenceFailure("candidate artifact is stale or expired")
    return parse_time(artifact.get("created_at"), "artifact created_at")


def select_latest_artifact(
    artifacts: Any,
    *,
    expected_sha: str,
    now: datetime,
    run_fetcher: Callable[[int], Mapping[str, Any]],
) -> tuple[Mapping[str, Any], int]:
    """Select the newest fresh artifact whose staging run is exact and successful."""

    expected_sha = require_sha(expected_sha, "expected candidate SHA")
    if not isinstance(artifacts, list):
        raise EvidenceFailure("artifact API response is not an array")
    candidates = [item for item in artifacts if isinstance(item, dict) and item.get("name") == ARTIFACT_NAME]
    if not candidates:
        raise EvidenceFailure("core-release-candidate artifact is missing")
    ranked: list[tuple[datetime, Mapping[str, Any], int]] = []
    stale = 0
    for artifact in candidates:
        try:
            created_at = _artifact_time(artifact, now)
        except EvidenceFailure:
            stale += 1
            continue
        workflow_run = artifact.get("workflow_run")
        if not isinstance(workflow_run, dict):
            continue
        try:
            run_id = require_run_id(workflow_run.get("id"), "artifact workflow run ID")
        except EvidenceFailure:
            continue
        run = run_fetcher(run_id)
        if (
            run.get("status") != "completed"
            or run.get("conclusion") != "success"
            or run.get("head_branch") != "staging"
            or run.get("path") != ".github/workflows/staging.yml"
            or str(run.get("head_sha", "")).lower() != expected_sha
        ):
            continue
        ranked.append((created_at, artifact, run_id))
    if not ranked:
        if stale == len(candidates):
            raise EvidenceFailure("all core-release-candidate artifacts are stale or expired")
        raise EvidenceFailure("no successful exact-head staging run owns a candidate artifact")
    ranked.sort(key=lambda item: item[0], reverse=True)
    _, artifact, run_id = ranked[0]
    return artifact, run_id


def fetch_artifacts(environment: Mapping[str, str]) -> list[Any]:
    repository = _repository(environment)
    payload = _json_request(
        environment,
        f"repos/{repository}/actions/artifacts?name={parse.quote(ARTIFACT_NAME)}&per_page=100",
    )
    if not isinstance(payload, dict) or not isinstance(payload.get("artifacts"), list):
        raise EvidenceFailure("artifact API response is malformed")
    return payload["artifacts"]


def fetch_run(environment: Mapping[str, str], run_id: int) -> Mapping[str, Any]:
    repository = _repository(environment)
    payload = _json_request(environment, f"repos/{repository}/actions/runs/{run_id}")
    if not isinstance(payload, dict):
        raise EvidenceFailure("workflow run API response is malformed")
    return payload


def fetch_source_version_and_tree(
    environment: Mapping[str, str],
    expected_sha: str,
) -> tuple[str, str]:
    repository = _repository(environment)
    expected_sha = require_sha(expected_sha, "expected candidate SHA")
    commit = _json_request(environment, f"repos/{repository}/commits/{expected_sha}")
    if not isinstance(commit, dict) or str(commit.get("sha", "")).lower() != expected_sha:
        raise EvidenceFailure("commit API did not return the exact candidate SHA")
    tree_sha = require_sha(nested(commit, "commit", "tree", "sha"), "candidate tree")
    versions: list[str] = []
    for manifest in ("apps/cli/Cargo.toml", "apps/api/Cargo.toml", "apps/agent/Cargo.toml"):
        content = _json_request(
            environment,
            f"repos/{repository}/contents/{manifest}?ref={parse.quote(expected_sha)}",
        )
        if not isinstance(content, dict) or content.get("encoding") != "base64":
            raise EvidenceFailure(f"{manifest} content API response is malformed")
        encoded = content.get("content")
        if not isinstance(encoded, str):
            raise EvidenceFailure(f"{manifest} content API response is malformed")
        try:
            # GitHub wraps Contents API base64 at fixed-width lines.  Strip
            # only whitespace, then retain strict alphabet/padding validation.
            text = base64.b64decode("".join(encoded.split()), validate=True).decode("utf-8")
        except (ValueError, UnicodeDecodeError) as exc:
            raise EvidenceFailure(f"{manifest} content is not valid base64 UTF-8") from exc
        match = re.search(r'^version = "([0-9]+\.[0-9]+\.[0-9]+)"$', text, re.MULTILINE)
        if not match:
            raise EvidenceFailure(f"{manifest} has no stable package version")
        versions.append(match.group(1))
    if len(set(versions)) != 1:
        raise EvidenceFailure("Core package versions do not match")
    return tree_sha, versions[0]


def fetch_commit_tree(environment: Mapping[str, str], expected_sha: str) -> str:
    repository = _repository(environment)
    expected_sha = require_sha(expected_sha, "expected merge SHA")
    commit = _json_request(environment, f"repos/{repository}/commits/{expected_sha}")
    if not isinstance(commit, dict) or str(commit.get("sha", "")).lower() != expected_sha:
        raise EvidenceFailure("merge commit API did not return the exact push SHA")
    return require_sha(nested(commit, "commit", "tree", "sha"), "merged tree")


def _receipt_from_archive(environment: Mapping[str, str], artifact: Mapping[str, Any]) -> bytes:
    artifact_id = require_run_id(artifact.get("id"), "candidate artifact ID")
    archive = _bytes_request(
        environment,
        f"repos/{_repository(environment)}/actions/artifacts/{artifact_id}/zip",
    )
    try:
        with zipfile.ZipFile(io.BytesIO(archive)) as bundle:
            names = [name for name in bundle.namelist() if Path(name).name == "core-release-candidate.json"]
            if len(names) != 1:
                raise EvidenceFailure("candidate artifact must contain one receipt")
            name = names[0]
            if name.startswith("/") or ".." in Path(name).parts:
                raise EvidenceFailure("candidate receipt archive path is unsafe")
            return bundle.read(name)
    except EvidenceFailure:
        raise
    except (zipfile.BadZipFile, KeyError, OSError) as exc:
        raise EvidenceFailure("candidate artifact is not a readable ZIP") from exc


def validate_candidate_artifact(
    environment: Mapping[str, str],
    *,
    expected_sha: str,
    expected_tree: str,
    expected_version: str,
    now: datetime,
) -> None:
    artifacts = fetch_artifacts(environment)
    artifact, run_id = select_latest_artifact(
        artifacts,
        expected_sha=expected_sha,
        now=now,
        run_fetcher=lambda candidate_run_id: fetch_run(environment, candidate_run_id),
    )
    receipt = _receipt_from_archive(environment, artifact)
    validator = Path(__file__).with_name("release-candidate.py")
    if not validator.is_file():
        raise EvidenceFailure("authoritative release-candidate validator is missing")
    with tempfile.TemporaryDirectory(prefix="core-release-evidence-") as temporary:
        receipt_path = Path(temporary) / "core-release-candidate.json"
        receipt_path.write_bytes(receipt)
        command = [
            sys.executable,
            str(validator),
            "validate",
            "--input",
            str(receipt_path),
            "--expected-sha",
            expected_sha,
            "--expected-tree",
            expected_tree,
            "--expected-version",
            expected_version,
            "--expected-staging-run-id",
            str(run_id),
            "--expected-candidate-run-id",
            str(run_id),
            "--max-age-seconds",
            str(MAX_AGE_SECONDS),
            "--now",
            now.astimezone(timezone.utc).replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ"),
        ]
        result = subprocess.run(command, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            detail = result.stderr.strip().splitlines()[-1] if result.stderr.strip() else "validator rejected receipt"
            raise EvidenceFailure(f"authoritative candidate validator failed: {detail}")


def validate_release_pr(environment: Mapping[str, str]) -> str:
    repository = _repository(environment)
    event_head = require_sha(required(environment, "EVENT_HEAD_SHA"), "event head")
    event_base = require_sha(required(environment, "EVENT_BASE_SHA"), "event base")
    event_head_ref = required(environment, "EVENT_HEAD_REF")
    event_base_ref = required(environment, "EVENT_BASE_REF")
    live_pull = fetch_live_pull(environment)
    version = validate_live_pull(
        live_pull,
        repository=repository,
        expected_base_ref=event_base_ref,
        expected_base_sha=event_base,
        expected_head_ref=event_head_ref,
        expected_head_sha=event_head,
    )
    if event_base_ref != "main" or version is None:
        if environment.get("EVENT_ACTION") != "labeled":
            raise EvidenceFailure("ordinary PR CI was not freshly approved")
        if environment.get("EVENT_LABEL") != environment.get("HOSTLET_APPROVAL_LABEL", ""):
            raise EvidenceFailure("ordinary PR trigger was not the homelab approval label")
        labels = live_pull.get("labels") if isinstance(live_pull, dict) else None
        if not isinstance(labels, list) or environment.get("HOSTLET_APPROVAL_LABEL", "") not in {
            item.get("name") for item in labels if isinstance(item, dict)
        }:
            raise EvidenceFailure("ordinary PR approval label is no longer present")
        validate_dependencies(json.loads(required(environment, "HOSTLET_PR_RESULTS_JSON")))
        return event_head
    expected_branch_version = RELEASE_BRANCH_PATTERN.fullmatch(event_head_ref)
    if expected_branch_version is None or expected_branch_version.group("version") != version:
        raise EvidenceFailure("release-candidate branch version does not match its live PR head")
    tree_sha, source_version = fetch_source_version_and_tree(environment, event_head)
    if source_version != version:
        raise EvidenceFailure("release-candidate branch version does not match Core manifests")
    validate_candidate_artifact(
        environment,
        expected_sha=event_head,
        expected_tree=tree_sha,
        expected_version=source_version,
        now=datetime.now(timezone.utc),
    )
    return event_head


def detect_main_reuse(environment: Mapping[str, str]) -> bool:
    """Return true only for a fully evidenced exact release-candidate merge."""

    if environment.get("GITHUB_EVENT_NAME") != "push" or environment.get("GITHUB_REF") != "refs/heads/main":
        return False
    push_sha = require_sha(required(environment, "GITHUB_SHA"), "main push SHA")
    pulls = fetch_commit_pulls(environment, push_sha)
    matches = [pull for pull in pulls if is_merged_release_pull(pull, repository=_repository(environment), push_sha=push_sha)]
    if len(matches) != 1:
        return False
    pull = matches[0]
    head_sha = require_sha(nested(pull, "head", "sha"), "merged release head")
    branch = nested(pull, "head", "ref")
    version = release_candidate_version(branch)
    if version is None:
        return False
    tree_sha, source_version = fetch_source_version_and_tree(environment, head_sha)
    if source_version != version:
        return False
    if fetch_commit_tree(environment, push_sha) != tree_sha:
        return False
    validate_candidate_artifact(
        environment,
        expected_sha=head_sha,
        expected_tree=tree_sha,
        expected_version=source_version,
        now=datetime.now(timezone.utc),
    )
    return True


def write_output(environment: Mapping[str, str], value: str) -> None:
    output_path = environment.get("GITHUB_OUTPUT", "")
    if output_path:
        with open(output_path, "a", encoding="utf-8") as output:
            output.write(f"reuse_heavy={value}\n")


def main() -> int:
    mode = os.environ.get("HOSTLET_RELEASE_EVIDENCE_MODE", "pr")
    try:
        if mode == "pr":
            head = validate_release_pr(os.environ)
            print(f"release candidate evidence passed for {head}")
            return 0
        if mode == "main":
            try:
                reusable = detect_main_reuse(os.environ)
                reason = "valid merged release candidate" if reusable else "ordinary or invalid release merge"
            except Exception as exc:  # main-push detection must fail open to heavy CI
                reusable = False
                reason = str(exc)
            write_output(os.environ, "true" if reusable else "false")
            print(f"main release evidence: reuse_heavy={'true' if reusable else 'false'} ({reason})")
            return 0
        raise EvidenceFailure(f"unsupported evidence mode: {mode}")
    except (EvidenceFailure, json.JSONDecodeError, OSError) as exc:
        print(f"release evidence failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
