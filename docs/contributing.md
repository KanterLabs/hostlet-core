# Contributing

Hostlet Core is open source self-hosted infrastructure. Contributions should preserve self-hosted behavior and keep hosted-service code out of the public repo.

## Local Checks

Run relevant checks before opening a change:

```bash
scripts/validate-local.sh
```

For narrower local runs, use the matching commands directly:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test --workspace
pnpm --dir apps/web lint
pnpm --dir apps/web build
docker compose -f infra/docker-compose.yml config
HOSTLET_IMAGE_TAG=v0.0.0 \
HOSTLET_API_IMAGE=ghcr.io/shanekanterman04/hostlet-api@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
HOSTLET_WEB_IMAGE=ghcr.io/shanekanterman04/hostlet-web@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
HOSTLET_AGENT_IMAGE=ghcr.io/shanekanterman04/hostlet-agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
HOSTLET_SCREENSHOTTER_IMAGE=ghcr.io/shanekanterman04/hostlet-screenshotter@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
docker compose -f infra/docker-compose.prod.yml config
```

Use narrower checks for small docs-only changes, but always run link and secret scans for documentation edits.

## Docs Rules

- Keep docs plain Markdown.
- Use **Hostlet** or **Hostlet Core** for the open-source self-hostable product.
- Do not add historical plans or versioned validation files back into `docs/`.
- Do not document secret values, internal-only IPs, provider IDs, private backup paths, billing config, private deployment config, or raw env files in tracked docs.
- Keep hosted-service production inventory in the private hosted-service repo, not here.

## Release Expectations

Releases start by finalizing `X.Y.Z` on `staging`, with the same version in
`apps/cli/Cargo.toml`, `apps/api/Cargo.toml`, and `apps/agent/Cargo.toml`. Select
the exact staging SHA and successful staging run for the
`.github/workflows/release-candidate.yml` workflow. That workflow runs
release-only checks, builds the Linux x86_64 CLI once, captures the four
existing immutable staging image digests (API, web, agent, and screenshotter),
and seals candidate proof with the CLI, SBOM, and provenance evidence. Those
artifacts are ready before publication and are never rebuilt during tagging.

Prepare the release PR from that exact staging head:

```bash
scripts/prepare-release-pr.sh X.Y.Z
```

The script verifies the version on `origin/staging`, rejects an existing release
tag, and creates or reuses `release-candidate/vX.Y.Z` only when the branch head
and pull-request head/base SHAs are exact. Do not merge until the release
candidate gate is successful.

The normal publication path is `hostlet release prepare`, followed by
`hostlet release promote`; use `hostlet release resume` to continue an
interrupted promotion. Publication is time-bounded and validates the sealed
candidate before aliasing the existing four image digests to `vX.Y.Z` and
uploading the identical stored CLI, checksum, SBOM, provenance, and candidate
receipt assets. It writes `hostlet-release.json` from that evidence; normal
publication performs no rebuild. The release tag and publication target are the
merged `main` commit, and publication requires its tree to equal the certified
candidate tree. Manual `git tag` and `git push --tags` releases are unsupported.

GitHub-generated notes categorize merged pull requests as features, fixes, or
other changes. They are a change summary, not proof that migration or rollback
considerations are absent. Release operators should add explicit breaking,
configuration, migration, and rollback guidance when applicable; the
`hostlet-release.json` migration flags remain the machine-readable source used
by the updater.

The published release contains:

- Linux x86_64 CLI binary and checksum
- `hostlet-release.json`
- Linux x86_64 GHCR images for API, web, agent, and screenshotter

## Security Review Expectations

Review carefully when touching:

- auth and session handling
- GitHub OAuth code
- encryption and secret handling
- API-to-agent job signing
- Docker/Caddy agent code
- database migrations
