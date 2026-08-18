# Core release-candidate receipt

`core-release-candidate.json` is the immutable handoff from the Core staging
candidate workflow to release publication and Hostlet Cloud. It records only
evidence that can be checked without rebuilding: the exact Core commit and
tree, release version, successful staging/candidate workflow runs, expiry,
four registry digest references, CLI artifact checksum, SBOM/provenance
references, and a self-checking fingerprint.

## Contract

The receipt uses schema `hostlet.core.release-candidate/v1` and is emitted as
UTF-8 JSON with sorted keys, compact separators, and one trailing newline.
The fingerprint is a lowercase SHA-256 hex string over that canonical JSON
with the `fingerprint` field removed. The CLI checksum is also lowercase
SHA-256 hex (without a `sha256:` prefix). Timestamps are UTC RFC3339 values in
`YYYY-MM-DDTHH:MM:SSZ` form; `version` is semantic-version text without a
leading `v`.

```json
{
  "schema": "hostlet.core.release-candidate/v1",
  "core": {"commit_sha": "<40 lowercase hex>", "tree_sha": "<40 lowercase hex>"},
  "version": "0.2.24",
  "workflows": {
    "staging": {"run_id": 12345, "job": "images", "conclusion": "success"},
    "candidate": {"run_id": 67890, "job": "seal-candidate", "conclusion": "success"}
  },
  "created_at": "2026-08-18T00:00:00Z",
  "expires_at": "2026-08-18T04:00:00Z",
  "images": {
    "api": "ghcr.io/<owner>/hostlet-api@sha256:<64 lowercase hex>",
    "web": "ghcr.io/<owner>/hostlet-web@sha256:<64 lowercase hex>",
    "agent": "ghcr.io/<owner>/hostlet-agent@sha256:<64 lowercase hex>",
    "screenshotter": "ghcr.io/<owner>/hostlet-screenshotter@sha256:<64 lowercase hex>"
  },
  "artifacts": {
    "cli": {"ref": "https://<durable-artifact-ref>", "sha256": "<64 lowercase hex>"},
    "sbom": {"ref": "https://<durable-sbom-ref>"},
    "provenance": {"ref": "https://<durable-provenance-ref>"}
  },
  "fingerprint": "<64 lowercase hex>"
}
```

The validator requires both workflow conclusions to be exactly `success` and
requires each `(run_id, job)` pair to identify different evidence. Candidate
proof is dispatched only for an explicitly selected staging SHA, so ordinary
staging pushes do not consume release-only heavy-runner capacity.
Mutable image tags, missing evidence, unknown fields, duplicate JSON keys,
stale expiry, and fingerprint drift are rejected. Image names must match their
map keys. Artifact references must use `https://`, `oci://`, or `artifact://`
and contain no whitespace.

## Workflow commands

Create a draft with all fields except `fingerprint`, then seal it into the
durable artifact name:

```bash
python3 scripts/release-candidate.py seal \
  --input dist/core-release-candidate.draft.json \
  --output dist/core-release-candidate.json \
  --expected-sha "$GITHUB_SHA" \
  --expected-tree "$CORE_TREE_SHA" \
  --expected-version "$VERSION" \
  --now "$NOW"
```

Every consumer should validate against its live exact facts. `--max-age-seconds`
checks the age from `created_at`; expiry is always checked independently.

```bash
python3 scripts/release-candidate.py validate \
  --input dist/core-release-candidate.json \
  --expected-sha "$GITHUB_SHA" \
  --expected-tree "$CORE_TREE_SHA" \
  --expected-version "$VERSION" \
  --expected-staging-run-id "$STAGING_RUN_ID" \
  --expected-candidate-run-id "$CANDIDATE_RUN_ID" \
  --max-age-seconds 14400
```

The command exits non-zero for every rejected condition and prints no receipt
contents on failure. `fingerprint --input` prints the validated fingerprint
for a downstream combined Cloud receipt to bind to Core evidence.
