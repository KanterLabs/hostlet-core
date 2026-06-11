use super::*;

/// Runs `git` against the checkout directory, streaming output as deployment logs.
pub(crate) async fn run_git(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    args: &[&str],
) -> anyhow::Result<()> {
    let mut full = vec!["-C", checkout.to_str().unwrap()];
    full.extend_from_slice(args);
    run_log(cfg, deployment_id, "git", &full).await
}

/// Checks out the requested ref: the branch tip (`commit_sha == "HEAD"`) is reset
/// to `FETCH_HEAD`, otherwise the exact commit is checked out detached. Shared by
/// the reuse and fresh-clone paths so the ref logic lives in one place.
pub(crate) async fn checkout_fetched_ref(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    branch: &str,
    commit_sha: &str,
) -> anyhow::Result<()> {
    if commit_sha == "HEAD" {
        run_git(
            cfg,
            deployment_id,
            checkout,
            &["checkout", "-B", branch, "FETCH_HEAD"],
        )
        .await
    } else {
        run_git(
            cfg,
            deployment_id,
            checkout,
            &["checkout", "--detach", commit_sha],
        )
        .await
    }
}

/// Ensures `checkout` contains the requested branch/commit, reusing an existing
/// clone when present or initializing a fresh one otherwise.  Self-heals three
/// failure modes:
///
/// 1. **Missing `.git`** (interrupted first clone, etc.) — wipes the checkout
///    directory and performs a fresh clone.
/// 2. **Remote mismatch** (the app's repo was changed) — wipes and reclones from
///    the new remote.  A fetch is never attempted into a checkout whose origin
///    differs from `expected_remote`.
/// 3. **Corrupt/locked state** (stale `index.lock`, partial fetch, etc.) — if the
///    fetch+checkout step on an otherwise-reusable checkout fails, the directory is
///    wiped and a fresh clone is attempted exactly once.
///
/// Only the exact `checkout` path is ever removed.
pub(crate) async fn sync_checkout(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    expected_remote: &str,
    fetch_remote: &str,
    branch: &str,
    commit_sha: &str,
) -> anyhow::Result<()> {
    if checkout.exists() {
        let git_dir_ok = checkout.join(".git").exists();
        let remote_ok = git_dir_ok
            && ensure_checkout_remote(cfg, deployment_id, checkout, expected_remote)
                .await
                .is_ok();

        if remote_ok {
            // Checkout appears reusable: try an in-place fetch + ref update.
            let reuse_result: anyhow::Result<()> = async {
                run_git(
                    cfg,
                    deployment_id,
                    checkout,
                    &["fetch", fetch_remote, branch],
                )
                .await?;
                checkout_fetched_ref(cfg, deployment_id, checkout, branch, commit_sha).await?;
                Ok(())
            }
            .await;

            match reuse_result {
                Ok(()) => return Ok(()),
                Err(err) => {
                    log(
                        cfg,
                        deployment_id,
                        "stderr",
                        &format!("Existing checkout failed ({err}); recreating from scratch."),
                    )
                    .await;
                    tokio::fs::remove_dir_all(checkout).await?;
                    // fall through to fresh path below
                }
            }
        } else {
            // Not reusable: log why, then wipe so the fresh path can proceed.
            let reason = if !git_dir_ok {
                "checkout directory exists but has no .git"
            } else {
                "existing checkout remote does not match the requested repository"
            };
            log(
                cfg,
                deployment_id,
                "stderr",
                &format!("Discarding checkout: {reason}; recreating from scratch."),
            )
            .await;
            tokio::fs::remove_dir_all(checkout).await?;
            // fall through to fresh path below
        }
    }

    // Fresh path: initialize, add remote, fetch, and check out.
    tokio::fs::create_dir_all(checkout).await?;
    run_git(cfg, deployment_id, checkout, &["init"]).await?;
    run_git(
        cfg,
        deployment_id,
        checkout,
        &["remote", "add", "origin", expected_remote],
    )
    .await?;
    run_git(
        cfg,
        deployment_id,
        checkout,
        &["fetch", fetch_remote, branch],
    )
    .await?;
    checkout_fetched_ref(cfg, deployment_id, checkout, branch, commit_sha).await
}

/// Reads `remote.origin.url` from the checkout's git config and verifies it
/// matches `expected_remote` (normalized: trailing `.git` and `https://` prefix
/// are stripped, then lowercased).
pub(crate) async fn ensure_checkout_remote(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    expected_remote: &str,
) -> anyhow::Result<()> {
    let remote = run_capture_trim(
        cfg,
        deployment_id,
        "git",
        &[
            "-C",
            checkout.to_str().unwrap(),
            "config",
            "--get",
            "remote.origin.url",
        ],
    )
    .await?;
    if normalize_git_remote(&remote) != normalize_git_remote(expected_remote) {
        bail!("existing checkout remote does not match the requested repository");
    }
    Ok(())
}

/// Verifies that HEAD in the checkout resolves to `expected_commit`.
pub(crate) async fn verify_git_head(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    expected_commit: &str,
) -> anyhow::Result<()> {
    let head = run_capture_trim(
        cfg,
        deployment_id,
        "git",
        &["-C", checkout.to_str().unwrap(), "rev-parse", "HEAD"],
    )
    .await?;
    if !head.eq_ignore_ascii_case(expected_commit) {
        bail!("checked-out commit did not match the signed deployment commit");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a minimal `Config` that points at a non-existent API so log posts
    /// fail fast (connect-refused) and are silently ignored via `post()`.
    fn test_config(workdir: PathBuf) -> Config {
        Config {
            api_url: "http://127.0.0.1:9".into(),
            http: http_client().unwrap(),
            server_id: Uuid::nil(),
            agent_token: "t".into(),
            job_signing_secret: "s".into(),
            workdir,
            local_mode: true,
            health_host: "127.0.0.1".into(),
            local_router: None,
        }
    }

    /// Run a git command in `path`, inheriting GIT_CONFIG_NOSYSTEM and supplying
    /// commit identity via env so tests are hermetic w.r.t. HOME/.gitconfig.
    fn git_cmd(path: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_EMAIL", "test@hostlet.test")
            .env("GIT_AUTHOR_NAME", "Hostlet Test")
            .env("GIT_COMMITTER_EMAIL", "test@hostlet.test")
            .env("GIT_COMMITTER_NAME", "Hostlet Test")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    /// Create a bare repo at `bare` seeded from a worktree at `work`.
    /// Returns the HEAD SHA of the initial commit.
    fn make_bare_origin(work: &Path, bare: &Path, filename: &str) -> String {
        std::fs::create_dir_all(work).unwrap();
        git_cmd(work, &["init"]);
        // Set branch name without requiring git >= 2.28 `init -b main`.
        git_cmd(work, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git_cmd(work, &["config", "user.email", "test@hostlet.test"]);
        git_cmd(work, &["config", "user.name", "Hostlet Test"]);
        std::fs::write(work.join(filename), filename).unwrap();
        git_cmd(work, &["add", "."]);
        git_cmd(work, &["commit", "-m", "initial"]);
        // Clone bare from the worktree — bare.git is what sync_checkout uses.
        let bare_str = bare.to_str().unwrap();
        git_cmd(
            work.parent().unwrap(),
            &["clone", "--bare", work.to_str().unwrap(), bare_str],
        );
        head_sha(bare)
    }

    /// Add a new file to the worktree and push to the bare repo.
    fn add_commit_and_push(work: &Path, bare: &Path, filename: &str) -> String {
        std::fs::write(work.join(filename), filename).unwrap();
        git_cmd(work, &["add", "."]);
        git_cmd(work, &["commit", "-m", &format!("add {filename}")]);
        git_cmd(work, &["push", "--force", bare.to_str().unwrap(), "main"]);
        head_sha(bare)
    }

    fn head_sha(repo: &Path) -> String {
        let output = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "rev-parse", "HEAD"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "git rev-parse HEAD failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn temp_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("hostlet-git-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    // --- tests ---

    #[tokio::test]
    async fn git_fresh_head_clone_checks_out_file() {
        let tmp = temp_root();
        let work = tmp.join("work");
        let bare = tmp.join("bare.git");
        make_bare_origin(&work, &bare, "hello.txt");
        let checkout = tmp.join("checkout");
        let cfg = test_config(tmp.clone());

        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();

        assert!(checkout.join("hello.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn git_picks_up_new_origin_commit() {
        let tmp = temp_root();
        let work = tmp.join("work");
        let bare = tmp.join("bare.git");
        make_bare_origin(&work, &bare, "file1.txt");
        let checkout = tmp.join("checkout");
        let cfg = test_config(tmp.clone());

        // First sync — only file1 present.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();
        assert!(checkout.join("file1.txt").exists());
        assert!(!checkout.join("file2.txt").exists());

        // Add a second commit to origin.
        add_commit_and_push(&work, &bare, "file2.txt");

        // Second sync — file2 should appear.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();
        assert!(checkout.join("file2.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn git_self_heals_missing_dot_git() {
        let tmp = temp_root();
        let work = tmp.join("work");
        let bare = tmp.join("bare.git");
        make_bare_origin(&work, &bare, "hello.txt");
        let checkout = tmp.join("checkout");
        let cfg = test_config(tmp.clone());

        // Initial checkout.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();

        // Simulate a corrupt checkout: remove .git but leave the working files.
        std::fs::remove_dir_all(checkout.join(".git")).unwrap();
        assert!(checkout.join("hello.txt").exists());
        assert!(!checkout.join(".git").exists());

        // sync_checkout must self-heal.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();
        assert!(checkout.join(".git").exists());
        assert!(checkout.join("hello.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn git_self_heals_changed_remote() {
        let tmp = temp_root();
        let work1 = tmp.join("work1");
        let bare1 = tmp.join("bare1.git");
        let work2 = tmp.join("work2");
        let bare2 = tmp.join("bare2.git");
        make_bare_origin(&work1, &bare1, "from-repo1.txt");
        make_bare_origin(&work2, &bare2, "from-repo2.txt");
        let checkout = tmp.join("checkout");
        let cfg = test_config(tmp.clone());

        // First sync uses bare1.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare1.to_str().unwrap(),
            bare1.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();
        assert!(checkout.join("from-repo1.txt").exists());

        // App repo changed to bare2 — sync_checkout must wipe and re-clone.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare2.to_str().unwrap(),
            bare2.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();
        assert!(
            checkout.join("from-repo2.txt").exists(),
            "checkout should contain files from the new repo"
        );
        assert!(
            !checkout.join("from-repo1.txt").exists(),
            "old repo files should be gone"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn git_self_heals_index_lock() {
        let tmp = temp_root();
        let work = tmp.join("work");
        let bare = tmp.join("bare.git");
        make_bare_origin(&work, &bare, "hello.txt");
        let checkout = tmp.join("checkout");
        let cfg = test_config(tmp.clone());

        // Initial checkout.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();

        // Create a stale index.lock that blocks `git checkout`.
        std::fs::write(checkout.join(".git/index.lock"), b"").unwrap();

        // sync_checkout must detect the failure and retry via fresh clone.
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            "HEAD",
        )
        .await
        .unwrap();
        assert!(checkout.join("hello.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn git_pinned_sha_checkout_passes_verify_git_head() {
        let tmp = temp_root();
        let work = tmp.join("work");
        let bare = tmp.join("bare.git");
        let first_sha = make_bare_origin(&work, &bare, "v1.txt");
        // Add a second commit so HEAD advances.
        add_commit_and_push(&work, &bare, "v2.txt");
        let checkout = tmp.join("checkout");
        let cfg = test_config(tmp.clone());

        // Sync with pinned first SHA (not HEAD).
        sync_checkout(
            &cfg,
            Uuid::nil(),
            &checkout,
            bare.to_str().unwrap(),
            bare.to_str().unwrap(),
            "main",
            &first_sha,
        )
        .await
        .unwrap();

        // verify_git_head must confirm the pinned commit is checked out.
        verify_git_head(&cfg, Uuid::nil(), &checkout, &first_sha)
            .await
            .unwrap();

        // The second file is NOT present because we checked out the first commit.
        assert!(checkout.join("v1.txt").exists());
        assert!(!checkout.join("v2.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
