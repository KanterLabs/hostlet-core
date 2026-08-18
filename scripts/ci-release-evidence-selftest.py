#!/usr/bin/env python3
"""Deterministic release PR and main-push evidence regression tests."""

from __future__ import annotations

import base64
import importlib.util
import json
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_PATH = ROOT / "scripts" / "ci-release-evidence.py"
HEAD = "a" * 40
TREE = "b" * 40
PUSH = "c" * 40
RUN_ID = 12345
NOW = datetime(2026, 8, 18, 2, 0, tzinfo=timezone.utc)


def load_evidence() -> ModuleType:
    spec = importlib.util.spec_from_file_location("ci_release_evidence", EVIDENCE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load release evidence evaluator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


evidence = load_evidence()


def expect_failure(name: str, callback: object) -> None:
    try:
        callback()  # type: ignore[operator]
    except evidence.EvidenceFailure:
        return
    raise AssertionError(f"{name}: expected fail-closed rejection")


def artifact(created_at: str = "2026-08-18T01:00:00Z", expires_at: str = "2026-08-18T05:00:00Z") -> dict[str, object]:
    return {
        "id": 987,
        "name": "core-release-candidate",
        "created_at": created_at,
        "expires_at": expires_at,
        "expired": False,
        "workflow_run": {"id": RUN_ID},
    }


def successful_run(head_sha: str = HEAD, conclusion: str = "success") -> dict[str, object]:
    return {
        "status": "completed",
        "conclusion": conclusion,
        "head_branch": "staging",
        "head_sha": head_sha,
        "path": ".github/workflows/staging.yml",
    }


def select(artifacts: object, run: dict[str, object] | None = None) -> tuple[dict[str, object], int]:
    return evidence.select_latest_artifact(  # type: ignore[return-value]
        artifacts,
        expected_sha=HEAD,
        now=NOW,
        run_fetcher=lambda _run_id: run or successful_run(),
    )


# Release vs ordinary PR classification is branch-bound, not commit-message-bound.
assert evidence.release_candidate_version("release-candidate/v0.2.25") == "0.2.25"
assert evidence.release_candidate_version("feature/release-candidate/v0.2.25") is None
assert evidence.release_candidate_version("release-candidate/v0.2.25-rc1") is None
ordinary_pull = {
    "state": "open",
    "base": {"ref": "main", "sha": "d" * 40},
    "head": {"ref": "feature/ordinary", "sha": HEAD, "repo": {"full_name": "KanterLabs/hostlet-core"}},
}
assert evidence.validate_live_pull(
    ordinary_pull,
    repository="KanterLabs/hostlet-core",
    expected_base_ref="main",
    expected_base_sha="d" * 40,
    expected_head_ref="feature/ordinary",
    expected_head_sha=HEAD,
) is None

passed = {name: {"result": "success"} for name in evidence.DEPENDENCIES}
passed[evidence.REVOCATION_DEPENDENCY] = {"result": "skipped"}
evidence.validate_dependencies(passed)
skipped = json.loads(json.dumps(passed))
skipped["rust"]["result"] = "skipped"
expect_failure("ordinary-approved-pr-skipped-old-job", lambda: evidence.validate_dependencies(skipped))

# Missing, stale, skipped, cancelled, and mismatched artifacts all fail closed.
expect_failure("missing-artifact", lambda: select([]))
expect_failure("stale-artifact", lambda: select([artifact(expires_at="2026-08-18T02:00:00Z")]))
expect_failure("skipped-staging-run", lambda: select([artifact()], successful_run(conclusion="skipped")))
expect_failure("cancelled-staging-run", lambda: select([artifact()], successful_run(conclusion="cancelled")))
expect_failure("mismatched-staging-head", lambda: select([artifact()], successful_run(head_sha=PUSH)))
assert select([artifact()])[1] == RUN_ID


# The GitHub Contents API wraps base64 payloads across lines.  Release PR
# evidence must accept that transport format without weakening base64 checks.
manifest_text = 'version = "0.2.25"\n'
manifest_base64 = base64.b64encode(manifest_text.encode("utf-8")).decode("ascii")
wrapped_manifest_base64 = "\n".join(
    manifest_base64[offset : offset + 8]
    for offset in range(0, len(manifest_base64), 8)
)
source_responses = iter(
    [
        {"sha": HEAD, "commit": {"tree": {"sha": TREE}}},
        *(
            {"encoding": "base64", "content": wrapped_manifest_base64}
            for _manifest in range(3)
        ),
    ]
)
with mock.patch.object(evidence, "_json_request", side_effect=lambda *_args, **_kwargs: next(source_responses)):
    assert evidence.fetch_source_version_and_tree(
        {"GITHUB_REPOSITORY": "KanterLabs/hostlet-core"}, HEAD
    ) == (TREE, "0.2.25")

invalid_source_responses = iter(
    [
        {"sha": HEAD, "commit": {"tree": {"sha": TREE}}},
        {"encoding": "base64", "content": "not base64!"},
    ]
)
with mock.patch.object(
    evidence,
    "_json_request",
    side_effect=lambda *_args, **_kwargs: next(invalid_source_responses),
):
    expect_failure(
        "invalid-source-base64",
        lambda: evidence.fetch_source_version_and_tree(
            {"GITHUB_REPOSITORY": "KanterLabs/hostlet-core"}, HEAD
        ),
    )


release_pull = {
    "merged_at": "2026-08-18T02:01:00Z",
    "merge_commit_sha": PUSH,
    "base": {"ref": "main", "repo": {"full_name": "KanterLabs/hostlet-core"}},
    "head": {
        "ref": "release-candidate/v0.2.25",
        "sha": HEAD,
        "repo": {"full_name": "KanterLabs/hostlet-core"},
    },
}
assert evidence.is_merged_release_pull(
    release_pull, repository="KanterLabs/hostlet-core", push_sha=PUSH
)
assert not evidence.is_merged_release_pull(
    {**release_pull, "merge_commit_sha": "e" * 40},
    repository="KanterLabs/hostlet-core",
    push_sha=PUSH,
)

base_environment = {
    "GITHUB_EVENT_NAME": "push",
    "GITHUB_REF": "refs/heads/main",
    "GITHUB_SHA": PUSH,
    "GITHUB_REPOSITORY": "KanterLabs/hostlet-core",
}
with mock.patch.object(evidence, "fetch_commit_pulls", return_value=[release_pull]), mock.patch.object(
    evidence, "fetch_source_version_and_tree", return_value=(TREE, "0.2.25")
), mock.patch.object(evidence, "fetch_commit_tree", return_value=TREE), mock.patch.object(
    evidence, "validate_candidate_artifact"
):
    assert evidence.detect_main_reuse(base_environment | {"GH_TOKEN": "test", "GITHUB_API_URL": "https://api.test"})

with mock.patch.object(evidence, "fetch_commit_pulls", return_value=[release_pull]), mock.patch.object(
    evidence, "fetch_source_version_and_tree", return_value=(TREE, "0.2.25")
), mock.patch.object(evidence, "fetch_commit_tree", return_value="d" * 40), mock.patch.object(
    evidence, "validate_candidate_artifact"
) as validate:
    assert not evidence.detect_main_reuse(
        base_environment | {"GH_TOKEN": "test", "GITHUB_API_URL": "https://api.test"}
    )
    validate.assert_not_called()

ordinary_environment = dict(base_environment, GITHUB_SHA=HEAD)
with mock.patch.object(evidence, "fetch_commit_pulls", return_value=[]):
    assert not evidence.detect_main_reuse(ordinary_environment | {"GH_TOKEN": "test", "GITHUB_API_URL": "https://api.test"})

scheduled = dict(base_environment, GITHUB_EVENT_NAME="schedule")
with mock.patch.object(evidence, "fetch_commit_pulls") as fetch:
    assert not evidence.detect_main_reuse(scheduled | {"GH_TOKEN": "test", "GITHUB_API_URL": "https://api.test"})
    fetch.assert_not_called()

# Bootstrap behavior is fail-open: the detector must never execute head-owned
# code when the trusted pre-push base predates this evaluator.
for workflow_name in ("ci.yml", "full-ci.yml", "deployability.yml"):
    workflow = (ROOT / ".github" / "workflows" / workflow_name).read_text(encoding="utf-8")
    assert "trusted release evidence evaluator is absent on the pre-push base" in workflow
    assert 'echo "reuse_heavy=false" >> "${GITHUB_OUTPUT}"' in workflow

print("ci-release-evidence self-test passed")
