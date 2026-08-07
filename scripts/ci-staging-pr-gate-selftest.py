#!/usr/bin/env python3
"""Negative and positive fixtures for ci-staging-pr-gate.py."""

from __future__ import annotations

import importlib.util
import json
from collections.abc import Callable
from pathlib import Path
from types import ModuleType
from unittest import mock
from urllib import error


ROOT = Path(__file__).resolve().parents[1]
GATE_PATH = ROOT / "scripts" / "ci-staging-pr-gate.py"
HEAD = "a" * 40
STALE_HEAD = "b" * 40
BASE = "c" * 40
STALE_BASE = "d" * 40
APPROVAL = "staging-homelab-ci-approved"
EXPECTED_DEPENDENCIES = (
    "secrets",
    "rust",
    "database",
    "web",
    "compose",
    "docker",
    "topology-e2e",
    "remote-build",
)


class FakeResponse:
    def __init__(self, payload: object, status: int = 200) -> None:
        self.payload = json.dumps(payload).encode()
        self.status = status

    def __enter__(self) -> FakeResponse:
        return self

    def __exit__(self, *_args: object) -> None:
        return None

    def read(self) -> bytes:
        return self.payload


def load_gate() -> ModuleType:
    spec = importlib.util.spec_from_file_location("ci_staging_pr_gate", GATE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load staging PR gate")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gate = load_gate()
assert gate.DEPENDENCIES == EXPECTED_DEPENDENCIES


def environment() -> dict[str, str]:
    return {
        "EVENT_ACTION": "labeled",
        "EVENT_BASE_SHA": BASE,
        "EVENT_LABEL": APPROVAL,
        "EVENT_HEAD_SHA": HEAD,
        "GH_TOKEN": "test-token",
        "GITHUB_API_URL": "https://api.github.test",
        "GITHUB_REPOSITORY": "KanterLabs/hostlet-core",
        "HOSTLET_STAGING_PR_APPROVAL_LABEL": APPROVAL,
        "PR_NUMBER": "17",
    }


def results() -> dict[str, dict[str, str]]:
    fixture = {name: {"result": "success"} for name in gate.DEPENDENCIES}
    fixture[gate.REVOCATION_DEPENDENCY] = {"result": "skipped"}
    return fixture


def live_pull() -> dict[str, object]:
    return {
        "state": "open",
        "base": {"ref": "staging", "sha": BASE},
        "head": {"sha": HEAD, "repo": {"full_name": "KanterLabs/hostlet-core"}},
        "labels": [{"name": APPROVAL}],
    }


def expect_failure(name: str, callback: Callable[[], object]) -> None:
    try:
        callback()
    except gate.GateFailure:
        return
    raise AssertionError(f"{name}: expected fail-closed result")


passing = environment()
passing["HOSTLET_STAGING_PR_RESULTS_JSON"] = json.dumps(results())
with mock.patch.object(gate.request, "urlopen", return_value=FakeResponse(live_pull())):
    assert gate.run(passing) == HEAD

for dependency in gate.DEPENDENCIES:
    for conclusion in ("skipped", "neutral", "failure", "cancelled"):
        fixture = results()
        fixture[dependency]["result"] = conclusion
        expect_failure(
            f"{dependency}-{conclusion}",
            lambda fixture=fixture: gate.validate_dependencies(fixture),
        )

    missing = results()
    missing.pop(dependency)
    expect_failure(
        f"missing-{dependency}",
        lambda missing=missing: gate.validate_dependencies(missing),
    )

unexpected = results()
unexpected["untrusted-job"] = {"result": "success"}
expect_failure("unexpected-dependency", lambda: gate.validate_dependencies(unexpected))

stale_base = live_pull()
stale_base["base"] = {"ref": "staging", "sha": STALE_BASE}
expect_failure("stale-live-base", lambda: gate.validate_live_pull(stale_base, environment()))

malformed_base = live_pull()
malformed_base["base"] = {"ref": "staging", "sha": "not-a-sha"}
expect_failure(
    "malformed-live-base",
    lambda: gate.validate_live_pull(malformed_base, environment()),
)

missing_base = live_pull()
missing_base["base"] = {"ref": "staging"}
expect_failure(
    "missing-live-base",
    lambda: gate.validate_live_pull(missing_base, environment()),
)

stale = live_pull()
stale["head"] = {"sha": STALE_HEAD, "repo": {"full_name": "KanterLabs/hostlet-core"}}
expect_failure("stale-live-head", lambda: gate.validate_live_pull(stale, environment()))

malformed = live_pull()
malformed["head"] = {"sha": "not-a-sha", "repo": {"full_name": "KanterLabs/hostlet-core"}}
expect_failure("malformed-live-head", lambda: gate.validate_live_pull(malformed, environment()))

unapproved = live_pull()
unapproved["labels"] = []
expect_failure("missing-approval", lambda: gate.validate_live_pull(unapproved, environment()))

api_failure = environment()
api_failure["HOSTLET_STAGING_PR_RESULTS_JSON"] = json.dumps(results())
with mock.patch.object(gate.request, "urlopen", side_effect=error.URLError("fixture failure")):
    expect_failure("api-failure", lambda: gate.run(api_failure))

bad_results = environment()
bad_results["HOSTLET_STAGING_PR_RESULTS_JSON"] = "{"
expect_failure("malformed-results", lambda: gate.run(bad_results))

print("ci-staging-pr-gate self-test passed")
