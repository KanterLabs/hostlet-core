#!/usr/bin/env python3
"""Deterministic regression tests for release-candidate.py."""

from __future__ import annotations

import copy
import importlib.util
import json
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType
from typing import Any, Callable


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "release-candidate.py"
NOW = "2026-08-18T02:00:00Z"
SHA = "a" * 40
TREE = "b" * 40
CHECKSUM = "c" * 64


def load_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("release_candidate", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load release candidate validator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


release = load_module()


def draft() -> dict[str, Any]:
    digest = "d" * 64
    return {
        "schema": release.SCHEMA,
        "core": {"commit_sha": SHA, "tree_sha": TREE},
        "version": "0.2.24",
        "workflows": {
            "staging": {"run_id": 12345, "conclusion": "success"},
            "candidate": {"run_id": 67890, "conclusion": "success"},
        },
        "created_at": "2026-08-18T00:00:00Z",
        "expires_at": "2026-08-18T04:00:00Z",
        "images": {
            name: f"ghcr.io/shanekanterman04/hostlet-{name}@sha256:{digest}"
            for name in release.IMAGE_NAMES
        },
        "artifacts": {
            "cli": {
                "ref": "https://github.com/KanterLabs/hostlet-core/actions/runs/67890/artifacts/123",
                "sha256": CHECKSUM,
            },
            "sbom": {"ref": "https://github.com/KanterLabs/hostlet-core/attestations/sbom-123"},
            "provenance": {
                "ref": "https://github.com/KanterLabs/hostlet-core/attestations/provenance-123"
            },
        },
    }


def expect_failure(name: str, callback: Callable[[], object]) -> None:
    try:
        callback()
    except release.ReceiptError:
        return
    raise AssertionError(f"{name}: expected fail-closed rejection")


def sealed() -> dict[str, Any]:
    return release.seal_receipt(draft(), now=NOW)


valid = sealed()
assert release.validate_receipt(
    valid,
    expected_sha=SHA,
    expected_tree=TREE,
    expected_version="0.2.24",
    expected_staging_run_id=12345,
    expected_candidate_run_id=67890,
    now=NOW,
    max_age_seconds=7200,
) == valid
assert release.canonical_json(valid) == release.canonical_json(json.loads(release.canonical_json(valid)))
assert len(valid["fingerprint"]) == 64
assert valid["fingerprint"] == release.fingerprint_for({key: value for key, value in valid.items() if key != "fingerprint"})

expect_failure("malformed-root", lambda: release.validate_receipt([]))
missing = copy.deepcopy(valid)
del missing["artifacts"]
expect_failure("missing-field", lambda: release.validate_receipt(missing, now=NOW))
unexpected = copy.deepcopy(valid)
unexpected["extra"] = True
expect_failure("unexpected-field", lambda: release.validate_receipt(unexpected, now=NOW))

bad_sha = copy.deepcopy(valid)
bad_sha["core"]["commit_sha"] = "not-a-sha"
expect_failure("malformed-sha", lambda: release.validate_receipt(bad_sha, now=NOW))
bad_tree = copy.deepcopy(valid)
bad_tree["core"]["tree_sha"] = "e" * 39
expect_failure("malformed-tree", lambda: release.validate_receipt(bad_tree, now=NOW))
expect_failure("sha-mismatch", lambda: release.validate_receipt(valid, expected_sha="f" * 40, now=NOW))
expect_failure("tree-mismatch", lambda: release.validate_receipt(valid, expected_tree="f" * 40, now=NOW))

bad_version = copy.deepcopy(valid)
bad_version["version"] = "v0.2.24"
expect_failure("malformed-version", lambda: release.validate_receipt(bad_version, now=NOW))
expect_failure("version-mismatch", lambda: release.validate_receipt(valid, expected_version="0.2.25", now=NOW))

stale = copy.deepcopy(valid)
stale["expires_at"] = "2026-08-18T02:00:00Z"
stale["fingerprint"] = release.fingerprint_for({key: value for key, value in stale.items() if key != "fingerprint"})
expect_failure("stale-expiry", lambda: release.validate_receipt(stale, now=NOW))
expect_failure(
    "stale-age",
    lambda: release.validate_receipt(valid, now="2026-08-18T03:00:01Z", max_age_seconds=3600),
)

bad_digest = copy.deepcopy(valid)
bad_digest["images"]["web"] = bad_digest["images"]["web"].replace("@sha256:" + "d" * 64, "@latest")
expect_failure("mutable-image", lambda: release.validate_receipt(bad_digest, now=NOW))
wrong_digest_name = copy.deepcopy(valid)
wrong_digest_name["images"]["api"] = wrong_digest_name["images"]["api"].replace("hostlet-api", "hostlet-web")
expect_failure("image-name-mismatch", lambda: release.validate_receipt(wrong_digest_name, now=NOW))

bad_checksum = copy.deepcopy(valid)
bad_checksum["artifacts"]["cli"]["sha256"] = "not-a-checksum"
expect_failure("checksum-mismatch", lambda: release.validate_receipt(bad_checksum, now=NOW))
missing_sbom = copy.deepcopy(valid)
del missing_sbom["artifacts"]["sbom"]
expect_failure("missing-sbom", lambda: release.validate_receipt(missing_sbom, now=NOW))

for workflow_name in ("staging", "candidate"):
    for conclusion in ("skipped", "cancelled", "failure", "neutral"):
        failed_workflow = copy.deepcopy(valid)
        failed_workflow["workflows"][workflow_name]["conclusion"] = conclusion
        expect_failure(
            f"{workflow_name}-{conclusion}",
            lambda failed_workflow=failed_workflow: release.validate_receipt(failed_workflow, now=NOW),
        )
same_run = copy.deepcopy(valid)
same_run["workflows"]["candidate"]["run_id"] = same_run["workflows"]["staging"]["run_id"]
expect_failure("duplicate-workflow-run", lambda: release.validate_receipt(same_run, now=NOW))
expect_failure("run-id-mismatch", lambda: release.validate_receipt(valid, expected_candidate_run_id=7, now=NOW))
fingerprint_mismatch = copy.deepcopy(valid)
fingerprint_mismatch["fingerprint"] = "e" * 64
expect_failure("fingerprint-mismatch", lambda: release.validate_receipt(fingerprint_mismatch, now=NOW))

with tempfile.TemporaryDirectory(prefix="release-candidate-selftest-") as temporary:
    temporary_path = Path(temporary)
    input_path = temporary_path / "draft.json"
    output_path = temporary_path / release.RECEIPT_NAME
    input_path.write_text(json.dumps(draft(), indent=2) + "\n", encoding="utf-8")
    subprocess.run(
        [
            "python3",
            str(MODULE_PATH),
            "seal",
            "--input",
            str(input_path),
            "--output",
            str(output_path),
            "--expected-sha",
            SHA,
            "--expected-tree",
            TREE,
            "--expected-version",
            "0.2.24",
            "--now",
            NOW,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert output_path.read_text(encoding="utf-8").endswith("\n")
    subprocess.run(
        [
            "python3",
            str(MODULE_PATH),
            "validate",
            "--input",
            str(output_path),
            "--expected-sha",
            SHA,
            "--expected-tree",
            TREE,
            "--expected-version",
            "0.2.24",
            "--expected-staging-run-id",
            "12345",
            "--expected-candidate-run-id",
            "67890",
            "--max-age-seconds",
            "7200",
            "--now",
            NOW,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    noncanonical = temporary_path / "noncanonical.json"
    noncanonical.write_text(json.dumps(json.loads(output_path.read_text()), indent=2) + "\n", encoding="utf-8")
    rejected = subprocess.run(
        ["python3", str(MODULE_PATH), "validate", "--input", str(noncanonical), "--now", NOW],
        capture_output=True,
        text=True,
    )
    assert rejected.returncode == 1

print("release-candidate self-test passed")
