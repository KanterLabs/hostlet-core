# AGENTS — hostlet-core

Public, self‑hosted Hostlet core (Rust `apps/{api,agent,cli}` + Next `apps/web`). Keep
hosted‑service billing, private provider config, company infrastructure, real secrets, and
production‑only deployment detail **out of this repo** — it is public. See `CONTRIBUTING.md`.

## Branches & releases

- **`staging` is the development branch.** Do work on `staging` (or a feature branch merged
  into it). Pushing `staging` runs `.github/workflows/staging.yml`: quality gates, then it
  builds and pushes the moving `hostlet-{api,web,agent}:staging` and immutable
  `:sha-<commit>` images to GHCR, then rings the downstream Hostlet Cloud staging deploy
  (a `repository_dispatch`). So a push to `staging` updates the Cloud staging environment.
- **`main` is the release branch.** Releases are git tags `vX.Y.Z` (`release.yml`). The tag
  **must match** the `version` in `apps/cli/Cargo.toml`, `apps/api/Cargo.toml`, and
  `apps/agent/Cargo.toml` — bump all three before tagging or the release fails. Releases
  publish `hostlet-{api,web,agent}:vX.Y.Z` (+sha) and the GitHub release with
  `hostlet-release.json`.
- **Downstream:** Hostlet Cloud consumes this repo as a git submodule — its `staging` branch
  tracks core `staging`; its `main` pins a core `vX.Y.Z` tag. Don't rewrite public history;
  don't force‑push shared branches.

## Validate before you push

```bash
scripts/validate-local.sh         # or the narrower checks:
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/hostlet-target cargo clippy --workspace --all-targets --all-features -- -D warnings
CARGO_TARGET_DIR=/tmp/hostlet-target cargo test --workspace
pnpm --dir apps/web lint && pnpm --dir apps/web build
```

Workflows: `.github/workflows/{ci,staging,release,full-ci}.yml`. Never add secrets or private
operational data (IPs, hosts, credentials) to tracked files.

> A push to `staging` is a deploy: it publishes `:staging` images and rings the downstream
> Hostlet Cloud staging deploy. Keep `staging` green.
