# AGENTS — hostlet-core

Public, self‑hosted Hostlet core (Rust `apps/{api,agent,cli}` + Next `apps/web`). Keep
hosted‑service billing, private provider config, company infrastructure, real secrets, and
production‑only deployment detail **out of this repo** — it is public. See `CONTRIBUTING.md`.

## Branches & releases

- **`staging` is the development branch.** Do work on `staging` (or a feature branch merged
  into it). Pushing `staging` runs `.github/workflows/staging.yml`: quality gates, then it
  builds and pushes the moving `hostlet-{api,web,agent,screenshotter}:staging` and immutable
  `:sha-<commit>` images to GHCR, then rings the downstream Hostlet Cloud staging deploy
  (a `repository_dispatch`). So a push to `staging` updates the Cloud staging environment.
- **`main` is the release branch.** Releases are git tags `vX.Y.Z` (`release.yml`). The tag
  **must match** the `version` in `apps/cli/Cargo.toml`, `apps/api/Cargo.toml`, and
  `apps/agent/Cargo.toml` — bump all three before tagging or the release fails. Releases
  publish `hostlet-{api,web,agent,screenshotter}:vX.Y.Z` (+sha) and the GitHub release with
  `hostlet-release.json`.
- **Downstream:** Hostlet Cloud consumes this repo as a git submodule — its `staging` branch
  tracks core `staging`; its `main` pins a core `vX.Y.Z` tag. Don't rewrite public history;
  don't force‑push shared branches.

## Overlay architecture and placement rules

**Core is a submodule consumed by hostlet-cloud (`vendor/hostlet-core`).** Cloud's
build overlays core files at file granularity — a same-named cloud file replaces the
core file wholesale; a file that has no cloud counterpart is inherited unchanged.

### API overlay (files cloud currently overrides — regenerate before trusting)

Cloud overrides mean the whole file is forked; shared helpers placed there will drift
independently in each repo.  **Shared helpers must NOT live in any of these files:**

```
apps/api/src/state.rs            apps/api/src/lib.rs
apps/api/src/github.rs           apps/api/src/github/inference.rs
apps/api/src/web/app_delete.rs   apps/api/src/web/app_env.rs
apps/api/src/web/apps.rs         apps/api/src/web/audit.rs
apps/api/src/web/cleanup.rs      apps/api/src/web/dns/cloudflare.rs
apps/api/src/web/dns/mod.rs      apps/api/src/web/health.rs
apps/api/src/web/jobs.rs         apps/api/src/web/mod.rs
apps/api/src/web/servers.rs      apps/api/src/web/system.rs
apps/api/src/web/validation.rs
```

Cloud-only files (additions, no core counterpart) are **not** overrides: `auth/`,
`cloud/`, `github_app.rs`, `github/{hooks,repos,status,webhook}.rs`,
`web/{billing,dto,portfolio_*,update_checks,version}*`.

**Where to put new shared Rust helpers:** `apps/api/src/env.rs` (env/config
utilities) or any other file not listed above. Cloud inherits these without forking.

### Web overlay

Cloud's web build (see `scripts/lib/cloud-web-ci.sh::cloud_web_prepare_overlay`):
1. Copies all of core's `apps/web` as the base.
2. **Deletes core's `app/` wholesale** (except `globals.css`), then copies cloud's `app/` in.
3. **Merges** cloud's `components/` over core's (cloud wins on conflicts).
4. **Merges** cloud's `public/` over core's (cloud wins on conflicts).
5. Inherits core's `lib/` untouched.

Placement rules:
- Core web shared helpers belong in `apps/web/lib/` — cloud inherits `lib/` untouched.
- A `lib/` module must **never** import from `@/app/...`; that path is replaced by
  cloud's `app/` and the import will break the cloud build.
- A `lib/` module must be self-contained or import only other `lib/` files and
  `components/` files cloud does not override (currently `GitHubStatus.tsx` and
  `Nav.tsx` are overridden — do not depend on them from `lib/`).

### Drift check (cloud CI)

`scripts/check-core-drift.sh` (in cloud) reports when any overridden file in core
changed since the pinned submodule SHA.  It is non-blocking (warning output, exit 0)
but should be reviewed before every submodule advance.

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
