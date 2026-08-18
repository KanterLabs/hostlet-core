#!/usr/bin/env python3
"""Seal and validate an immutable Core release-candidate receipt.

The receipt is deliberately a small, dependency-free evidence module.  A
workflow writes an unsigned draft, ``seal`` emits canonical JSON plus its
fingerprint, and every later consumer calls ``validate`` with the facts it
expects.  Validation is fail-closed: unknown fields, duplicate JSON keys,
non-canonical bytes, stale evidence, mutable image references, and every
non-success workflow conclusion are rejected.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
from collections.abc import Mapping
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA = "hostlet.core.release-candidate/v1"
RECEIPT_NAME = "core-release-candidate.json"
IMAGE_NAMES = ("api", "web", "agent", "screenshotter")
TOP_LEVEL_FIELDS = frozenset(
    {
        "schema",
        "core",
        "version",
        "workflows",
        "created_at",
        "expires_at",
        "images",
        "artifacts",
        "fingerprint",
    }
)
CORE_FIELDS = frozenset({"commit_sha", "tree_sha"})
WORKFLOW_FIELDS = frozenset({"run_id", "job", "conclusion"})
ARTIFACT_FIELDS = frozenset({"cli", "sbom", "provenance"})
CLI_FIELDS = frozenset({"ref", "sha256"})
REF_FIELDS = frozenset({"ref"})
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
HEX_PATTERN = re.compile(r"^[0-9a-f]{64}$")
VERSION_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
IMAGE_PATTERN = re.compile(
    r"^[a-z0-9]+(?:[._-][a-z0-9]+)*(?::[0-9]+)?/"
    r"(?:[a-z0-9]+(?:(?:[._-]|/)[a-z0-9]+)*)@sha256:[0-9a-f]{64}$"
)
UTC_TIMESTAMP_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
REF_PATTERN = re.compile(r"^(?:https://|oci://|artifact://)[^\s]+$")


class ReceiptError(ValueError):
    """Expected fail-closed receipt validation failure."""


def canonical_json(value: Any) -> str:
    """Return the one JSON representation used by the receipt contract."""

    try:
        return json.dumps(
            value,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        )
    except (TypeError, ValueError) as exc:
        raise ReceiptError("receipt contains a non-canonical JSON value") from exc


def _object_no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ReceiptError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def parse_json_bytes(raw: bytes, label: str = "receipt") -> Any:
    """Parse strict UTF-8 JSON, rejecting duplicate keys and non-finite values."""

    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ReceiptError(f"{label} is not UTF-8 JSON") from exc
    try:
        return json.loads(
            text,
            object_pairs_hook=_object_no_duplicates,
            parse_constant=lambda value: (_ for _ in ()).throw(
                ReceiptError(f"{label} contains non-finite JSON value {value}")
            ),
        )
    except ReceiptError:
        raise
    except (json.JSONDecodeError, TypeError) as exc:
        raise ReceiptError(f"{label} is malformed JSON") from exc


def load_receipt(path: Path, *, require_canonical: bool = True) -> dict[str, Any]:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise ReceiptError(f"could not read {path}: {type(exc).__name__}") from exc
    value = parse_json_bytes(raw, str(path))
    if not isinstance(value, dict):
        raise ReceiptError("receipt root must be a JSON object")
    if require_canonical:
        expected = canonical_json(value).encode("utf-8")
        if raw != expected and raw != expected + b"\n":
            raise ReceiptError("receipt JSON is not canonical sorted JSON")
    return value


def _require_mapping(value: Any, label: str) -> Mapping[str, Any]:
    if not isinstance(value, dict):
        raise ReceiptError(f"{label} must be a JSON object")
    return value


def _require_fields(value: Mapping[str, Any], fields: frozenset[str], label: str) -> None:
    actual = set(value)
    missing = sorted(fields - actual)
    unexpected = sorted(actual - fields)
    if missing:
        raise ReceiptError(f"{label} is missing field(s): {', '.join(missing)}")
    if unexpected:
        raise ReceiptError(f"{label} has unexpected field(s): {', '.join(unexpected)}")


def _require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ReceiptError(f"{label} must be a non-empty string")
    if any(ord(char) < 0x20 or char.isspace() for char in value):
        raise ReceiptError(f"{label} contains whitespace or control characters")
    return value


def _require_sha(value: Any, label: str) -> str:
    value = _require_string(value, label)
    if not SHA_PATTERN.fullmatch(value):
        raise ReceiptError(f"{label} must be a lowercase 40-character commit SHA")
    return value


def _require_version(value: Any) -> str:
    value = _require_string(value, "version")
    if not VERSION_PATTERN.fullmatch(value):
        raise ReceiptError("version must be canonical semantic version text without a leading v")
    return value


def parse_timestamp(value: Any, label: str) -> datetime:
    value = _require_string(value, label)
    if not UTC_TIMESTAMP_PATTERN.fullmatch(value):
        raise ReceiptError(f"{label} must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise ReceiptError(f"{label} is not a valid UTC timestamp") from exc
    return parsed


def _require_run_id(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise ReceiptError(f"{label} must be a positive integer")
    return value


def _require_ref(value: Any, label: str) -> str:
    value = _require_string(value, label)
    if not REF_PATTERN.fullmatch(value):
        raise ReceiptError(f"{label} must be an HTTPS, OCI, or artifact reference")
    return value


def _require_checksum(value: Any) -> str:
    value = _require_string(value, "artifacts.cli.sha256")
    if not HEX_PATTERN.fullmatch(value):
        raise ReceiptError("artifacts.cli.sha256 must be a lowercase 64-character SHA-256 checksum")
    return value


def _validate_images(value: Any) -> dict[str, str]:
    images = _require_mapping(value, "images")
    if set(images) != set(IMAGE_NAMES):
        missing = sorted(set(IMAGE_NAMES) - set(images))
        unexpected = sorted(set(images) - set(IMAGE_NAMES))
        details = []
        if missing:
            details.append("missing " + ", ".join(missing))
        if unexpected:
            details.append("unexpected " + ", ".join(unexpected))
        raise ReceiptError("images has " + "; ".join(details))

    result: dict[str, str] = {}
    for name in IMAGE_NAMES:
        reference = _require_string(images[name], f"images.{name}")
        if not IMAGE_PATTERN.fullmatch(reference):
            raise ReceiptError(f"images.{name} must be an immutable registry digest reference")
        image_name = reference.split("@", 1)[0].rsplit("/", 1)[-1]
        if image_name != f"hostlet-{name}":
            raise ReceiptError(f"images.{name} points to {image_name}, not hostlet-{name}")
        result[name] = reference
    return result


def _validate_workflows(value: Any) -> dict[str, dict[str, Any]]:
    workflows = _require_mapping(value, "workflows")
    if set(workflows) != {"staging", "candidate"}:
        raise ReceiptError("workflows must contain exactly staging and candidate")
    result: dict[str, dict[str, Any]] = {}
    for name in ("staging", "candidate"):
        record = _require_mapping(workflows[name], f"workflows.{name}")
        _require_fields(record, WORKFLOW_FIELDS, f"workflows.{name}")
        run_id = _require_run_id(record["run_id"], f"workflows.{name}.run_id")
        job = _require_string(record["job"], f"workflows.{name}.job")
        conclusion = _require_string(record["conclusion"], f"workflows.{name}.conclusion")
        if conclusion != "success":
            raise ReceiptError(f"workflows.{name}.conclusion must be success (got {conclusion})")
        result[name] = {"conclusion": conclusion, "job": job, "run_id": run_id}
    staging_identity = (result["staging"]["run_id"], result["staging"]["job"])
    candidate_identity = (result["candidate"]["run_id"], result["candidate"]["job"])
    if staging_identity == candidate_identity:
        raise ReceiptError("staging and candidate workflow evidence must identify different jobs")
    return result


def _validate_artifacts(value: Any) -> dict[str, Any]:
    artifacts = _require_mapping(value, "artifacts")
    _require_fields(artifacts, ARTIFACT_FIELDS, "artifacts")
    cli = _require_mapping(artifacts["cli"], "artifacts.cli")
    _require_fields(cli, CLI_FIELDS, "artifacts.cli")
    result: dict[str, Any] = {
        "cli": {
            "ref": _require_ref(cli["ref"], "artifacts.cli.ref"),
            "sha256": _require_checksum(cli["sha256"]),
        }
    }
    for name in ("sbom", "provenance"):
        record = _require_mapping(artifacts[name], f"artifacts.{name}")
        _require_fields(record, REF_FIELDS, f"artifacts.{name}")
        result[name] = {"ref": _require_ref(record["ref"], f"artifacts.{name}.ref")}
    return result


def fingerprint_for(receipt: Mapping[str, Any]) -> str:
    """Hash canonical receipt evidence while excluding its self-referential field."""

    unsigned = dict(receipt)
    unsigned.pop("fingerprint", None)
    return hashlib.sha256(canonical_json(unsigned).encode("utf-8")).hexdigest()


def _validate_unsigned(receipt: Mapping[str, Any]) -> dict[str, Any]:
    _require_fields(receipt, TOP_LEVEL_FIELDS - {"fingerprint"}, "receipt")
    if receipt["schema"] != SCHEMA:
        raise ReceiptError(f"schema must be {SCHEMA}")
    core = _require_mapping(receipt["core"], "core")
    _require_fields(core, CORE_FIELDS, "core")
    normalized_core = {
        "commit_sha": _require_sha(core["commit_sha"], "core.commit_sha"),
        "tree_sha": _require_sha(core["tree_sha"], "core.tree_sha"),
    }
    version = _require_version(receipt["version"])
    created_at = parse_timestamp(receipt["created_at"], "created_at")
    expires_at = parse_timestamp(receipt["expires_at"], "expires_at")
    if expires_at <= created_at:
        raise ReceiptError("expires_at must be later than created_at")
    normalized_workflows = _validate_workflows(receipt["workflows"])
    normalized_images = _validate_images(receipt["images"])
    normalized_artifacts = _validate_artifacts(receipt["artifacts"])
    return {
        "schema": SCHEMA,
        "core": normalized_core,
        "version": version,
        "workflows": normalized_workflows,
        "created_at": created_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "expires_at": expires_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "images": normalized_images,
        "artifacts": normalized_artifacts,
    }


def _normalise_now(value: datetime | str | None) -> datetime:
    if value is None:
        return datetime.now(timezone.utc).replace(microsecond=0)
    if isinstance(value, datetime):
        if value.tzinfo is None:
            raise ReceiptError("now must be timezone-aware UTC")
        return value.astimezone(timezone.utc)
    return parse_timestamp(value, "now")


def validate_receipt(
    receipt: Any,
    *,
    expected_sha: str | None = None,
    expected_tree: str | None = None,
    expected_version: str | None = None,
    expected_staging_run_id: int | None = None,
    expected_candidate_run_id: int | None = None,
    expected_fingerprint: str | None = None,
    max_age_seconds: int | None = None,
    now: datetime | str | None = None,
) -> dict[str, Any]:
    """Validate and return normalized receipt evidence.

    Expected values are optional so the same interface serves sealing and
    consumers.  Workflows should provide every known exact SHA, tree, version,
    run ID, and a fixed ``now`` when replaying evidence deterministically.
    """

    mapping = _require_mapping(receipt, "receipt")
    actual_fields = set(mapping)
    if actual_fields != TOP_LEVEL_FIELDS:
        missing = sorted(TOP_LEVEL_FIELDS - actual_fields)
        unexpected = sorted(actual_fields - TOP_LEVEL_FIELDS)
        details = []
        if missing:
            details.append("missing " + ", ".join(missing))
        if unexpected:
            details.append("unexpected " + ", ".join(unexpected))
        raise ReceiptError("receipt fields: " + "; ".join(details))
    normalized = _validate_unsigned({key: value for key, value in mapping.items() if key != "fingerprint"})
    fingerprint = _require_string(mapping["fingerprint"], "fingerprint")
    if not HEX_PATTERN.fullmatch(fingerprint):
        raise ReceiptError("fingerprint must be a lowercase 64-character SHA-256 value")
    expected_fingerprint_value = fingerprint_for(normalized)
    if fingerprint != expected_fingerprint_value:
        raise ReceiptError("fingerprint does not match canonical receipt evidence")

    if expected_sha is not None and normalized["core"]["commit_sha"] != _require_sha(expected_sha, "expected SHA"):
        raise ReceiptError("receipt Core commit SHA does not match expected SHA")
    if expected_tree is not None and normalized["core"]["tree_sha"] != _require_sha(expected_tree, "expected tree"):
        raise ReceiptError("receipt Core tree SHA does not match expected tree")
    if expected_version is not None and normalized["version"] != _require_version(expected_version):
        raise ReceiptError("receipt version does not match expected version")
    if expected_staging_run_id is not None and normalized["workflows"]["staging"]["run_id"] != _require_run_id(
        expected_staging_run_id, "expected staging run ID"
    ):
        raise ReceiptError("receipt staging workflow run ID does not match expected run ID")
    if expected_candidate_run_id is not None and normalized["workflows"]["candidate"]["run_id"] != _require_run_id(
        expected_candidate_run_id, "expected candidate run ID"
    ):
        raise ReceiptError("receipt candidate workflow run ID does not match expected run ID")
    if expected_fingerprint is not None and fingerprint != _require_checksum(expected_fingerprint):
        raise ReceiptError("receipt fingerprint does not match expected fingerprint")

    current = _normalise_now(now)
    created_at = parse_timestamp(normalized["created_at"], "created_at")
    expires_at = parse_timestamp(normalized["expires_at"], "expires_at")
    if created_at > current:
        raise ReceiptError("receipt created_at is in the future")
    if expires_at <= current:
        raise ReceiptError("receipt has expired")
    if max_age_seconds is not None:
        if isinstance(max_age_seconds, bool) or not isinstance(max_age_seconds, int) or max_age_seconds < 0:
            raise ReceiptError("max_age_seconds must be a non-negative integer")
        if (current - created_at).total_seconds() > max_age_seconds:
            raise ReceiptError("receipt is older than max_age_seconds")
    return {**normalized, "fingerprint": fingerprint}


def seal_receipt(
    draft: Any,
    *,
    expected_sha: str | None = None,
    expected_tree: str | None = None,
    expected_version: str | None = None,
    now: datetime | str | None = None,
) -> dict[str, Any]:
    """Validate an unsigned draft and add its immutable canonical fingerprint."""

    mapping = _require_mapping(draft, "draft")
    if "fingerprint" in mapping:
        raise ReceiptError("draft must not already contain a fingerprint")
    normalized = _validate_unsigned(mapping)
    current = _normalise_now(now)
    created_at = parse_timestamp(normalized["created_at"], "created_at")
    expires_at = parse_timestamp(normalized["expires_at"], "expires_at")
    if created_at > current:
        raise ReceiptError("draft created_at is in the future")
    if expires_at <= current:
        raise ReceiptError("draft has expired")
    if expected_sha is not None and normalized["core"]["commit_sha"] != _require_sha(expected_sha, "expected SHA"):
        raise ReceiptError("draft Core commit SHA does not match expected SHA")
    if expected_tree is not None and normalized["core"]["tree_sha"] != _require_sha(expected_tree, "expected tree"):
        raise ReceiptError("draft Core tree SHA does not match expected tree")
    if expected_version is not None and normalized["version"] != _require_version(expected_version):
        raise ReceiptError("draft version does not match expected version")
    sealed = {**normalized, "fingerprint": fingerprint_for(normalized)}
    return sealed


def write_canonical(path: Path, value: Mapping[str, Any]) -> None:
    data = (canonical_json(value) + "\n").encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}.", delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except OSError as exc:
        if "temporary" in locals():
            temporary.unlink(missing_ok=True)
        raise ReceiptError(f"could not write {path}: {type(exc).__name__}") from exc


def _timestamp_argument(value: str) -> str:
    parse_timestamp(value, "timestamp")
    return value


def _positive_id_argument(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("run ID must be an integer") from exc
    if parsed < 1:
        raise argparse.ArgumentTypeError("run ID must be positive")
    return parsed


def _nonnegative_int_argument(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("value must be an integer") from exc
    if parsed < 0:
        raise argparse.ArgumentTypeError("value must be non-negative")
    return parsed


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    subparsers = root.add_subparsers(dest="command", required=True)

    seal = subparsers.add_parser("seal", help="seal an unsigned draft receipt")
    seal.add_argument("--input", required=True, type=Path)
    seal.add_argument("--output", required=True, type=Path)
    seal.add_argument("--expected-sha", "--expected-commit-sha", dest="expected_sha")
    seal.add_argument("--expected-tree", "--expected-tree-sha", dest="expected_tree")
    seal.add_argument("--expected-version")
    seal.add_argument("--now", type=_timestamp_argument)

    validate = subparsers.add_parser("validate", help="validate a sealed receipt")
    validate.add_argument("--input", required=True, type=Path)
    validate.add_argument("--expected-sha", "--expected-commit-sha", dest="expected_sha")
    validate.add_argument("--expected-tree", "--expected-tree-sha", dest="expected_tree")
    validate.add_argument("--expected-version")
    validate.add_argument("--expected-staging-run-id", type=_positive_id_argument)
    validate.add_argument("--expected-candidate-run-id", type=_positive_id_argument)
    validate.add_argument("--expected-fingerprint")
    validate.add_argument("--max-age-seconds", type=_nonnegative_int_argument)
    validate.add_argument("--now", type=_timestamp_argument)

    fingerprint = subparsers.add_parser("fingerprint", help="print a sealed receipt fingerprint")
    fingerprint.add_argument("--input", required=True, type=Path)
    return root


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "seal":
            draft = load_receipt(args.input, require_canonical=False)
            sealed = seal_receipt(
                draft,
                expected_sha=args.expected_sha,
                expected_tree=args.expected_tree,
                expected_version=args.expected_version,
                now=args.now,
            )
            write_canonical(args.output, sealed)
            print(f"sealed {args.output} fingerprint={sealed['fingerprint']}")
            return 0
        receipt = load_receipt(args.input)
        if args.command == "fingerprint":
            validated = validate_receipt(receipt)
            print(validated["fingerprint"])
            return 0
        validate_receipt(
            receipt,
            expected_sha=args.expected_sha,
            expected_tree=args.expected_tree,
            expected_version=args.expected_version,
            expected_staging_run_id=args.expected_staging_run_id,
            expected_candidate_run_id=args.expected_candidate_run_id,
            expected_fingerprint=args.expected_fingerprint,
            max_age_seconds=args.max_age_seconds,
            now=args.now,
        )
        print(f"validated {args.input} fingerprint={receipt['fingerprint']}")
        return 0
    except ReceiptError as exc:
        print(f"release candidate validation failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
