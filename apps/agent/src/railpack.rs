use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const RAILPACK_BUILDKIT_CONTAINER: &str = "hostlet-railpack-buildkit";
const DEFAULT_RAILPACK_BUILDKIT_IMAGE: &str = "moby/buildkit:buildx-stable-1";
const DEFAULT_RAILPACK_BUILDKIT_IDLE_SECONDS: u64 = 1_800;
const DEFAULT_RAILPACK_BUILDKIT_READY_TIMEOUT_SECS: u64 = 30;
const RAILPACK_BUILDKIT_READY_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct RailpackBuildResult {
    pub(crate) runtime_metadata: Value,
}

struct RailpackBuildkitSession {
    host: String,
    container: Option<String>,
}

impl RailpackBuildkitSession {
    async fn stop_after_build(&self, cfg: &Config, deployment_id: Uuid) {
        let Some(container) = self.container.clone() else {
            // External BUILDKIT_HOST — we don't manage its lifecycle.
            return;
        };
        let idle = {
            let refcounts = buildkit_refcounts();
            let mut counts = refcounts.lock().expect("buildkit refcounts mutex poisoned");
            buildkit_release(&mut counts, &container)
        };
        if !idle {
            // Another build is still using this container; leave it running.
            return;
        }
        if railpack_buildkit_keepalive() {
            schedule_railpack_buildkit_idle_stop(cfg.clone(), deployment_id, container);
            return;
        }
        let _ = run_log(cfg, deployment_id, "docker", &["stop", &container]).await;
    }
}

// --- per-container in-use reference counts ---

type BuildkitRefcountMap = Arc<Mutex<HashMap<String, usize>>>;

fn buildkit_refcounts() -> BuildkitRefcountMap {
    static COUNTS: OnceLock<BuildkitRefcountMap> = OnceLock::new();
    COUNTS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

/// Increment the in-use count for `container`. Pure: operates on an owned map.
fn buildkit_acquire(counts: &mut HashMap<String, usize>, container: &str) {
    *counts.entry(container.to_string()).or_insert(0) += 1;
}

/// Decrement the in-use count for `container`. Returns `true` when the count
/// reaches zero (container is idle). Unknown containers are considered idle.
/// Pure: operates on an owned map.
fn buildkit_release(counts: &mut HashMap<String, usize>, container: &str) -> bool {
    match counts.get_mut(container) {
        Some(n) if *n > 1 => {
            *n -= 1;
            false
        }
        Some(_) => {
            counts.remove(container);
            true
        }
        None => true,
    }
}

pub(crate) fn should_try_railpack(err: &anyhow::Error) -> bool {
    err.to_string() == NO_NATIVE_BUILD_PLAN
}

pub(crate) async fn railpack_build_app(
    cfg: &Config,
    deployment_id: Uuid,
    app_name: &str,
    image: &str,
    project_dir: &Path,
    port: i64,
    p: &Value,
) -> anyhow::Result<RailpackBuildResult> {
    let railpack = std::env::var("HOSTLET_RAILPACK_BIN").unwrap_or_else(|_| "railpack".into());
    let buildkit = ensure_railpack_buildkit(cfg, deployment_id).await?;
    log(
        cfg,
        deployment_id,
        "stdout",
        "Building generated runtime with Railpack.",
    )
    .await;
    let build_started = Instant::now();
    let args = railpack_build_args(image, app_name, port, project_dir, p)?;
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let build_result = run_log_in_dir_env(
        cfg,
        deployment_id,
        project_dir,
        &[("BUILDKIT_HOST", buildkit.host.as_str())],
        &railpack,
        &arg_refs,
    )
    .await;
    buildkit.stop_after_build(cfg, deployment_id).await;
    build_result.with_context(|| format!("Railpack failed to build {}", project_dir.display()))?;
    let build_duration_ms = build_started.elapsed().as_millis();
    let image_size = image_size_bytes(image).await.ok();
    if let Some(size) = image_size {
        log(
            cfg,
            deployment_id,
            "stdout",
            &format!("Built image size: {size} bytes."),
        )
        .await;
    }
    Ok(RailpackBuildResult {
        runtime_metadata: railpack_runtime_metadata(build_duration_ms, image_size),
    })
}

pub(crate) fn railpack_runtime_metadata(
    build_duration_ms: u128,
    image_size_bytes: Option<i64>,
) -> Value {
    image_budget_runtime_metadata(
        json!({
            "packagingStrategy": "generated",
            "generatedDockerfile": false,
            "buildBackend": "railpack",
            "detectedLanguage": null,
            "detectedFramework": null,
            "runtimeKind": null,
            "packageManager": null,
            "buildDurationMs": build_duration_ms,
            "imageSizeBytes": image_size_bytes,
        }),
        image_size_bytes,
    )
}

fn railpack_build_args(
    image: &str,
    cache_key: &str,
    port: i64,
    project_dir: &Path,
    p: &Value,
) -> anyhow::Result<Vec<String>> {
    let mut args = vec![
        "build".to_string(),
        "--name".to_string(),
        image.to_string(),
        "--progress".to_string(),
        "plain".to_string(),
        "--cache-key".to_string(),
        cache_key.to_string(),
        "--env".to_string(),
        format!("PORT={port}"),
        "--error-missing-start".to_string(),
    ];
    if let Some(command) = payload_command(p, "build_command") {
        validate_dockerfile_command(&command)?;
        args.push("--build-cmd".to_string());
        args.push(command);
    }
    if let Some(command) = payload_command(p, "start_command") {
        validate_dockerfile_command(&command)?;
        args.push("--start-cmd".to_string());
        args.push(command);
    }
    args.push(
        project_dir
            .to_str()
            .with_context(|| format!("path is not valid UTF-8: {}", project_dir.display()))?
            .to_string(),
    );
    Ok(args)
}

async fn ensure_railpack_buildkit(
    cfg: &Config,
    deployment_id: Uuid,
) -> anyhow::Result<RailpackBuildkitSession> {
    if let Ok(host) = std::env::var("BUILDKIT_HOST") {
        if !host.trim().is_empty() {
            // External BuildKit host — caller manages its lifecycle; no refcount.
            return Ok(RailpackBuildkitSession {
                host,
                container: None,
            });
        }
    }
    let container = railpack_buildkit_container();
    let output = command_output(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", container.as_str()],
        Duration::from_secs(30),
    )
    .await;
    match output {
        Ok(output) if output.status.success() => {
            if String::from_utf8_lossy(&output.stdout).trim() != "true" {
                run_log(cfg, deployment_id, "docker", &["start", container.as_str()]).await?;
            }
        }
        _ => {
            let image = railpack_buildkit_image();
            let args = railpack_buildkit_run_args(&image, &container);
            let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
            if let Err(run_err) = run_log(cfg, deployment_id, "docker", &arg_refs).await {
                // `docker run` failed — re-inspect to see if a concurrent deploy already
                // created the container (race on missing container → both try to run).
                let probe = command_output(
                    "docker",
                    &["inspect", "-f", "{{.State.Running}}", container.as_str()],
                    Duration::from_secs(30),
                )
                .await;
                match probe {
                    Ok(p) if p.status.success() => {
                        // Container exists; check whether it is running.
                        if String::from_utf8_lossy(&p.stdout).trim() != "true" {
                            // Exists but stopped (e.g. exited after a previous session).
                            run_log(cfg, deployment_id, "docker", &["start", container.as_str()])
                                .await?;
                        }
                        // else: already running — proceed.
                    }
                    _ => return Err(run_err),
                }
            }
        }
    }
    // BuildKit's daemon starts asynchronously after `docker run`/`docker start`;
    // probe readiness on every path (a concurrent deploy may have just created
    // the container) before acquiring the refcount, so a failed probe never
    // leaks a count.
    wait_for_railpack_buildkit_ready(&container).await?;
    // Acquire the refcount *before* returning the session so stop_after_build
    // has an accurate count even if the caller drops the session early.
    {
        let refcounts = buildkit_refcounts();
        let mut counts = refcounts.lock().expect("buildkit refcounts mutex poisoned");
        buildkit_acquire(&mut counts, &container);
    }
    Ok(RailpackBuildkitSession {
        host: format!("docker-container://{container}"),
        container: Some(container),
    })
}

/// Waits for buildkitd inside `container` to accept connections.
///
/// `docker run`/`docker start` return before buildkitd is listening, so the
/// first build after a cold start would otherwise fail to connect. Probed
/// even when the container was already running: a concurrent deploy may have
/// created it moments ago.
async fn wait_for_railpack_buildkit_ready(container: &str) -> anyhow::Result<()> {
    let timeout_secs = railpack_buildkit_ready_timeout_secs();
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let probe = command_output(
            "docker",
            &["exec", container, "buildctl", "debug", "workers"],
            Duration::from_secs(10),
        )
        .await;
        if matches!(&probe, Ok(output) if output.status.success()) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "buildkitd in container {container} did not become ready within {timeout_secs} seconds; `docker exec {container} buildctl debug workers` kept failing"
            );
        }
        tokio::time::sleep(RAILPACK_BUILDKIT_READY_POLL_INTERVAL).await;
    }
}

fn railpack_buildkit_container() -> String {
    std::env::var("HOSTLET_RAILPACK_BUILDKIT_CONTAINER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| RAILPACK_BUILDKIT_CONTAINER.to_string())
}

fn railpack_buildkit_image() -> String {
    railpack_buildkit_image_value(
        std::env::var("HOSTLET_RAILPACK_BUILDKIT_IMAGE")
            .ok()
            .as_deref(),
    )
}

/// Returns `value` if non-empty and non-whitespace; otherwise the default image.
/// Empty strings must fall back to the default so that `VAR=` in compose
/// (passthrough with unset host var) behaves identically to an absent var.
fn railpack_buildkit_image_value(value: Option<&str>) -> String {
    value
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(DEFAULT_RAILPACK_BUILDKIT_IMAGE)
        .to_string()
}

fn railpack_buildkit_keepalive() -> bool {
    let value = std::env::var("HOSTLET_RAILPACK_BUILDKIT_KEEPALIVE").ok();
    railpack_buildkit_keepalive_value(value.as_deref())
}

fn railpack_buildkit_keepalive_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

fn railpack_buildkit_idle_seconds() -> u64 {
    std::env::var("HOSTLET_RAILPACK_BUILDKIT_IDLE_SECONDS")
        .ok()
        .as_deref()
        .and_then(railpack_buildkit_idle_seconds_value)
        .unwrap_or(DEFAULT_RAILPACK_BUILDKIT_IDLE_SECONDS)
}

fn railpack_buildkit_idle_seconds_value(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok().filter(|seconds| *seconds > 0)
}

fn railpack_buildkit_ready_timeout_secs() -> u64 {
    std::env::var("HOSTLET_RAILPACK_BUILDKIT_READY_TIMEOUT_SECS")
        .ok()
        .as_deref()
        .and_then(railpack_buildkit_ready_timeout_secs_value)
        .unwrap_or(DEFAULT_RAILPACK_BUILDKIT_READY_TIMEOUT_SECS)
}

fn railpack_buildkit_ready_timeout_secs_value(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok().filter(|seconds| *seconds > 0)
}

fn railpack_buildkit_memory_limit_mb() -> Option<u64> {
    std::env::var("HOSTLET_RAILPACK_BUILDKIT_MEMORY_LIMIT_MB")
        .ok()
        .as_deref()
        .and_then(railpack_buildkit_memory_limit_mb_value)
}

fn railpack_buildkit_memory_limit_mb_value(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok().filter(|mb| *mb >= 128)
}

fn railpack_buildkit_run_args(image: &str, container: &str) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        container.to_string(),
        "--privileged".to_string(),
    ];
    if let Some(memory_mb) = railpack_buildkit_memory_limit_mb() {
        args.push("--memory".to_string());
        args.push(format!("{memory_mb}m"));
    }
    args.push(image.to_string());
    args
}

type BuildkitUseMap = Arc<Mutex<HashMap<String, Instant>>>;

fn railpack_buildkit_last_used() -> BuildkitUseMap {
    static LAST_USED: OnceLock<BuildkitUseMap> = OnceLock::new();
    LAST_USED
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

fn schedule_railpack_buildkit_idle_stop(cfg: Config, deployment_id: Uuid, container: String) {
    let last_used = railpack_buildkit_last_used();
    let used_at = Instant::now();
    {
        let mut map = last_used.lock().expect("buildkit last-used mutex poisoned");
        map.insert(container.clone(), used_at);
    }
    let idle = Duration::from_secs(railpack_buildkit_idle_seconds());
    tokio::spawn(async move {
        tokio::time::sleep(idle).await;
        // Stop only if no newer build has touched the container AND no build is
        // currently using it (refcount guard prevents stopping mid-build).
        let should_stop = {
            let map = last_used.lock().expect("buildkit last-used mutex poisoned");
            let refcounts = buildkit_refcounts();
            let counts = refcounts.lock().expect("buildkit refcounts mutex poisoned");
            map.get(&container).is_some_and(|latest| *latest == used_at)
                && counts.get(&container).is_none_or(|n| *n == 0)
        };
        if should_stop {
            let _ = run_log(&cfg, deployment_id, "docker", &["stop", &container]).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_packaging_miss_falls_through_to_railpack() {
        assert!(should_try_railpack(&anyhow::anyhow!(NO_NATIVE_BUILD_PLAN)));
        assert!(!should_try_railpack(&anyhow::anyhow!(
            "packaging strategy dockerfile requires a Dockerfile at the app root"
        )));
    }

    #[test]
    fn railpack_args_include_cache_port_and_overrides() {
        let args = railpack_build_args(
            "hostlet/test:latest",
            "demo-app",
            4173,
            Path::new("/tmp/demo-app"),
            &json!({
                "build_command": "python -m compileall .",
                "start_command": "python main.py"
            }),
        )
        .unwrap();
        assert_eq!(args[0], "build");
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--name", "hostlet/test:latest"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--cache-key", "demo-app"]));
        assert!(args.windows(2).any(|pair| pair == ["--env", "PORT=4173"]));
        assert!(args.contains(&"--error-missing-start".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--build-cmd", "python -m compileall ."]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--start-cmd", "python main.py"]));
        assert_eq!(args.last().unwrap(), "/tmp/demo-app");
    }

    #[test]
    fn railpack_buildkit_container_can_be_overridden_for_ci() {
        std::env::set_var(
            "HOSTLET_RAILPACK_BUILDKIT_CONTAINER",
            "hostlet-buildkit-ci-123",
        );
        assert_eq!(railpack_buildkit_container(), "hostlet-buildkit-ci-123");
        std::env::remove_var("HOSTLET_RAILPACK_BUILDKIT_CONTAINER");
    }

    #[test]
    fn railpack_buildkit_image_defaults_when_unset_or_empty() {
        // unset → default
        assert_eq!(
            railpack_buildkit_image_value(None),
            DEFAULT_RAILPACK_BUILDKIT_IMAGE
        );
        // empty string → default (VAR= passthrough with unset host var)
        assert_eq!(
            railpack_buildkit_image_value(Some("")),
            DEFAULT_RAILPACK_BUILDKIT_IMAGE
        );
        // whitespace-only → default
        assert_eq!(
            railpack_buildkit_image_value(Some("   ")),
            DEFAULT_RAILPACK_BUILDKIT_IMAGE
        );
    }

    #[test]
    fn railpack_buildkit_image_uses_exact_value_when_set() {
        assert_eq!(
            railpack_buildkit_image_value(Some("custom/buildkit:v0.12")),
            "custom/buildkit:v0.12"
        );
        assert_eq!(
            railpack_buildkit_image_value(Some("moby/buildkit:v0.19.0")),
            "moby/buildkit:v0.19.0"
        );
    }

    #[test]
    fn railpack_buildkit_keepalive_defaults_to_stop_after_build() {
        assert!(!railpack_buildkit_keepalive_value(None));
        assert!(!railpack_buildkit_keepalive_value(Some("")));
        assert!(!railpack_buildkit_keepalive_value(Some("false")));
    }

    #[test]
    fn railpack_buildkit_keepalive_accepts_true_values() {
        assert!(railpack_buildkit_keepalive_value(Some("1")));
        assert!(railpack_buildkit_keepalive_value(Some("true")));
        assert!(railpack_buildkit_keepalive_value(Some(" YES ")));
    }

    #[test]
    fn railpack_buildkit_idle_seconds_defaults_and_rejects_invalid_values() {
        assert_eq!(railpack_buildkit_idle_seconds_value("45"), Some(45));
        assert_eq!(railpack_buildkit_idle_seconds_value(" 1800 "), Some(1_800));
        assert_eq!(railpack_buildkit_idle_seconds_value("0"), None);
        assert_eq!(railpack_buildkit_idle_seconds_value(""), None);
        assert_eq!(railpack_buildkit_idle_seconds_value("soon"), None);
    }

    #[test]
    fn railpack_buildkit_ready_timeout_defaults_and_rejects_invalid_values() {
        assert_eq!(railpack_buildkit_ready_timeout_secs_value("30"), Some(30));
        assert_eq!(railpack_buildkit_ready_timeout_secs_value(" 45 "), Some(45));
        assert_eq!(railpack_buildkit_ready_timeout_secs_value("0"), None);
        assert_eq!(railpack_buildkit_ready_timeout_secs_value(""), None);
        assert_eq!(railpack_buildkit_ready_timeout_secs_value("soon"), None);
    }

    #[test]
    fn railpack_buildkit_memory_limit_requires_reasonable_mb() {
        assert_eq!(railpack_buildkit_memory_limit_mb_value("512"), Some(512));
        assert_eq!(railpack_buildkit_memory_limit_mb_value(" 256 "), Some(256));
        assert_eq!(railpack_buildkit_memory_limit_mb_value("127"), None);
        assert_eq!(railpack_buildkit_memory_limit_mb_value(""), None);
        assert_eq!(railpack_buildkit_memory_limit_mb_value("large"), None);
    }

    #[test]
    fn railpack_buildkit_run_args_include_optional_memory_limit() {
        std::env::set_var("HOSTLET_RAILPACK_BUILDKIT_MEMORY_LIMIT_MB", "512");

        let args = railpack_buildkit_run_args("moby/buildkit:buildx-stable-1", "hostlet-buildkit");

        assert!(args.windows(2).any(|pair| pair == ["--memory", "512m"]));
        assert_eq!(args.last().unwrap(), "moby/buildkit:buildx-stable-1");
        std::env::remove_var("HOSTLET_RAILPACK_BUILDKIT_MEMORY_LIMIT_MB");
    }

    #[test]
    fn railpack_runtime_metadata_records_unknown_image_size() {
        let metadata = railpack_runtime_metadata(1_500, None);

        assert_eq!(metadata["packagingStrategy"], "generated");
        assert_eq!(metadata["buildBackend"], "railpack");
        assert_eq!(metadata["buildDurationMs"], 1_500);
        assert!(metadata["imageSizeBytes"].is_null());
        assert_eq!(metadata["imageBudgetStatus"], "unknown");
    }

    #[test]
    fn buildkit_refcount_two_acquires_then_one_release_is_not_idle() {
        let mut counts = HashMap::new();
        buildkit_acquire(&mut counts, "hostlet-railpack-buildkit");
        buildkit_acquire(&mut counts, "hostlet-railpack-buildkit");
        let idle = buildkit_release(&mut counts, "hostlet-railpack-buildkit");
        assert!(!idle, "second acquire is still held");
        assert_eq!(counts.get("hostlet-railpack-buildkit"), Some(&1));
    }

    #[test]
    fn buildkit_refcount_second_release_makes_container_idle() {
        let mut counts = HashMap::new();
        buildkit_acquire(&mut counts, "hostlet-railpack-buildkit");
        buildkit_acquire(&mut counts, "hostlet-railpack-buildkit");
        buildkit_release(&mut counts, "hostlet-railpack-buildkit");
        let idle = buildkit_release(&mut counts, "hostlet-railpack-buildkit");
        assert!(idle, "both acquires released → idle");
        assert!(!counts.contains_key("hostlet-railpack-buildkit"));
    }

    #[test]
    fn buildkit_refcount_release_of_unknown_container_is_idle_and_does_not_panic() {
        let mut counts = HashMap::new();
        let idle = buildkit_release(&mut counts, "hostlet-railpack-buildkit");
        assert!(idle, "unknown container treated as idle");
    }
}
