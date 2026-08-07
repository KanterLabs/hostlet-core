#!/usr/bin/env python3
"""Fail-closed evaluator for the staging pull-request aggregate gate."""

from __future__ import annotations

import json
import os
import re
import sys
from collections.abc import Mapping
from typing import Any
from urllib import error, request


DEPENDENCIES = (
    "secrets",
    "rust",
    "database",
    "web",
    "compose",
    "docker",
    "topology-e2e",
    "remote-build",
)
REVOCATION_DEPENDENCY = "revoke-approval-on-update"
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY_PATTERN = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


class GateFailure(RuntimeError):
    """Expected validation failure that must keep the aggregate gate red."""


def required_env(environment: Mapping[str, str], name: str) -> str:
    value = environment.get(name, "")
    if not value:
        raise GateFailure(f"required environment value {name} is missing")
    return value


def require_sha(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA_PATTERN.fullmatch(value):
        raise GateFailure(f"{label} is not a full lowercase commit SHA")
    return value


def validate_dependencies(payload: Any) -> None:
    if not isinstance(payload, dict):
        raise GateFailure("dependency results are not a JSON object")
    unexpected = sorted(set(payload) - {*DEPENDENCIES, REVOCATION_DEPENDENCY})
    if unexpected:
        raise GateFailure("dependency results contain unexpected jobs: " + ", ".join(unexpected))
    failures = []
    for name in DEPENDENCIES:
        result_record = payload.get(name)
        result = result_record.get("result") if isinstance(result_record, dict) else None
        if result != "success":
            failures.append(f"{name}={result or 'missing'}")
    if failures:
        raise GateFailure("required dependencies did not succeed: " + ", ".join(failures))


def nested(mapping: Any, *keys: str) -> Any:
    value = mapping
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def validate_live_pull(pull: Any, environment: Mapping[str, str]) -> str:
    if not isinstance(pull, dict):
        raise GateFailure("live pull-request response is not a JSON object")

    expected_label = required_env(environment, "HOSTLET_STAGING_PR_APPROVAL_LABEL")
    repository = required_env(environment, "GITHUB_REPOSITORY")
    event_head = require_sha(required_env(environment, "EVENT_HEAD_SHA"), "event head")
    event_base = require_sha(required_env(environment, "EVENT_BASE_SHA"), "event base")
    if environment.get("EVENT_ACTION") != "labeled":
        raise GateFailure("staging approval was not freshly applied")
    if environment.get("EVENT_LABEL") != expected_label:
        raise GateFailure("triggering label was not the staging approval")
    if pull.get("state") != "open":
        raise GateFailure("pull request is not open")
    if nested(pull, "base", "ref") != "staging":
        raise GateFailure("pull request no longer targets staging")
    live_base = require_sha(nested(pull, "base", "sha"), "live pull-request base")
    if live_base != event_base:
        raise GateFailure("pull request base changed during validation")

    live_head = require_sha(nested(pull, "head", "sha"), "live pull-request head")
    if live_head != event_head:
        raise GateFailure("pull request head changed during validation")
    if nested(pull, "head", "repo", "full_name") != repository:
        raise GateFailure("pull request head is not in the Core repository")

    labels = pull.get("labels")
    if not isinstance(labels, list):
        raise GateFailure("live pull-request labels are malformed")
    label_names = {
        item.get("name") for item in labels if isinstance(item, dict) and isinstance(item.get("name"), str)
    }
    if expected_label not in label_names:
        raise GateFailure("staging approval is no longer present")
    return live_head


def fetch_live_pull(environment: Mapping[str, str]) -> Any:
    token = required_env(environment, "GH_TOKEN")
    api_url = required_env(environment, "GITHUB_API_URL").rstrip("/")
    repository = required_env(environment, "GITHUB_REPOSITORY")
    number = required_env(environment, "PR_NUMBER")
    if not REPOSITORY_PATTERN.fullmatch(repository):
        raise GateFailure("GITHUB_REPOSITORY is malformed")
    if not number.isdigit() or int(number) < 1:
        raise GateFailure("PR_NUMBER is malformed")

    api_request = request.Request(
        f"{api_url}/repos/{repository}/pulls/{number}",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with request.urlopen(api_request, timeout=30) as response:
            if response.status != 200:
                raise GateFailure(f"pull-request API returned HTTP {response.status}")
            payload = response.read()
    except GateFailure:
        raise
    except (error.HTTPError, error.URLError, TimeoutError, OSError) as exc:
        raise GateFailure(f"pull-request API request failed: {type(exc).__name__}") from exc
    try:
        return json.loads(payload)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise GateFailure("pull-request API returned malformed JSON") from exc


def run(environment: Mapping[str, str]) -> str:
    raw_results = required_env(environment, "HOSTLET_STAGING_PR_RESULTS_JSON")
    try:
        results = json.loads(raw_results)
    except json.JSONDecodeError as exc:
        raise GateFailure("dependency results contain malformed JSON") from exc
    validate_dependencies(results)
    return validate_live_pull(fetch_live_pull(environment), environment)


def main() -> int:
    try:
        head = run(os.environ)
    except GateFailure as exc:
        print(f"staging PR gate failed: {exc}", file=sys.stderr)
        return 1
    print(f"staging PR gate passed for {head}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
