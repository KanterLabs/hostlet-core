use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;
pub mod compose;
pub mod crypto;
mod inference;
mod protocol;
mod topology;
mod validation;
pub use inference::{
    compose_inspection, detect_start_command, dockerfile_inspection, gitea_inspection,
    infer_addons_from_compose, infer_dockerfile, infer_package_json, infer_package_manager,
    infer_service_addons, manifest_dependency_tokens, node_inspection, package_json_dependencies,
    railpack_inspection, unknown_inspection, with_command_suggestion, with_detected_services,
    CommandSuggestion, DetectedServices, DockerfileInference, PackageInference, RepoCommandFiles,
};
pub use protocol::*;
pub use topology::{
    attach_topology_plan, plan_repository_topology, validate_generated_topology_config,
    GeneratedTopologyConfig, HealthProbe, HealthProbeKind, InferenceConfidence, InferredRouting,
    InferredService, RepositoryFile, RepositoryInventory, ServiceCandidate, ServiceRole,
    TopologyPlan, TopologyReadiness, DEFAULT_BACKEND_PATH_PREFIXES,
    GENERATED_TOPOLOGY_SCHEMA_VERSION,
};
pub use validation::{
    clean_hostlet_config_path, clean_runtime_config, dangerous_host_process_env_key, domain_host,
    valid_app_name, valid_cpu_limit, valid_host_process_env_key, valid_memory_limit,
    validate_env_pairs,
};

/// Defines a `snake_case` status enum whose wire string is shared by serde, the
///
/// Each variant lists its canonical string exactly once, so `as_str` and
/// `from_str` can never drift apart (the previous hand-written impls duplicated
/// every arm twice and had to be kept in sync by hand). The `#[serde(rename_all
/// = "snake_case")]` attribute is applied so the JSON wire shape stays identical
/// to the canonical strings, which are the same values persisted in Postgres.
macro_rules! string_status_enum {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "snake_case")]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        impl $name {
            /// Returns the canonical wire/database string for this status.
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $wire ),+
                }
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ();

            fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
                match value {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err(()),
                }
            }
        }
    };
}

/// Validates a GitHub `owner/repo` identifier.
///
/// Mirrors GitHub's naming rules: exactly one `/` separator, each segment
/// non-empty and at most 100 chars, restricted to alphanumerics plus `.`, `_`
/// and `-`, and never beginning or ending with a dot.
pub fn valid_repo_full_name(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    [owner, repo].into_iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && !part.starts_with('.')
            && !part.ends_with('.')
    })
}

/// Validates a Git branch (ref) name against the subset of `git check-ref-format`
/// rules that matter for deploys.
///
/// Rejects empty names, names longer than 255 chars, a leading `-` (which Git
/// would treat as an option), leading/trailing `/`, the `..` and `@{` sequences
/// that Git forbids in refs, and any character outside alphanumerics plus
/// `/ . _ -`.
pub fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("@{")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

/// Validates a deploy domain, which is either a bare hostname or `host:port`.
///
/// When a trailing `:port` is present the port must be a non-empty, parseable
/// `u16` (the range Postgres/TCP allow); the host portion is checked with
/// [`valid_hostname`].
pub fn valid_domain(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return valid_hostname(value);
    };
    valid_hostname(host) && !port.is_empty() && port.parse::<u16>().is_ok()
}

/// Validates a DNS hostname per RFC 1035/1123 length and character limits.
///
/// The full name is capped at 253 chars and each dot-separated label at 63;
/// labels must be non-empty, may not start or end with `-`, and may contain only
/// alphanumerics and `-`. Leading/trailing dots are rejected.
pub fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Validates an HTTP health-check path. Must be an absolute path (leading `/`),
/// at most 256 chars, and free of
/// control characters and backslashes (which would be ambiguous or unsafe in a
/// URL path).
pub fn valid_health_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

pub fn valid_root_directory(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.split('/').any(|part| part == "..")
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

pub fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub fn valid_env_value(value: &str) -> bool {
    value.len() <= 65_536
}

/// Validates a Hostlet-managed Docker container name: the `hostlet-` prefix, at
/// most 128 chars, and restricted to alphanumerics plus `- _ .`.
pub fn valid_container_name(value: &str) -> bool {
    value.starts_with("hostlet-")
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Derives a URL-safe slug from an app name: lowercases alphanumeric characters,
/// replaces all other characters with `-`, trims leading/trailing `-`, and
/// returns `"app"` when the result would be empty. Used for default domain labels
/// and other places where a user-visible identifier is derived from a free-text
/// app name.
pub fn app_slug(value: &str) -> String {
    let slug = value
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "app".into()
    } else {
        slug
    }
}

pub fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn clean_command(value: Option<String>) -> Result<Option<String>, &'static str> {
    let Some(value) = clean_optional(value) else {
        return Ok(None);
    };
    if value.len() > 500 || value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err("commands cannot contain newlines, NUL bytes, or more than 500 characters");
    }
    Ok(Some(value))
}

/// Validate and normalise an optional command patch from an update request.
///
/// The wire type for install/build/start commands on an update is
/// `Option<Option<String>>`: the outer `None` means "leave the column
/// untouched", an inner `None` means "clear the column", and an inner `Some`
/// carries the new value which is validated and trimmed via [`clean_command`].
/// The caller-visible shape (`None` / `Some(None)` / `Some(Some(_))`) is
/// preserved so persistence can distinguish "unchanged" from "cleared".
///
/// Extracted from the previously private `clean_command_field` (core) and
/// `validate_optional_command` (cloud overlay), which were byte-identical.
pub fn clean_optional_command(
    field: Option<Option<String>>,
) -> Result<Option<Option<String>>, &'static str> {
    match field {
        Some(Some(value)) => Ok(Some(clean_command(Some(value))?)),
        Some(None) => Ok(Some(None)),
        None => Ok(None),
    }
}

pub fn clean_runtime_kind(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("single");
    match value {
        "single" | "compose" => Ok(value.to_string()),
        _ => Err("runtime kind must be single or compose"),
    }
}

pub fn clean_packaging_strategy(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("auto");
    match value {
        "auto" | "dockerfile" | "generated" => Ok(value.to_string()),
        _ => Err("packaging strategy must be auto, dockerfile, or generated"),
    }
}

pub fn parse_github_repo(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches(".git");
    if let Some(repo) = trimmed
        .strip_prefix("git@github.com:")
        .and_then(parse_owner_repo)
    {
        return Some(repo);
    }
    if let Ok(url) = url::Url::parse(trimmed) {
        if url.host_str()? != "github.com" {
            return None;
        }
        return parse_owner_repo(url.path().trim_start_matches('/'));
    }
    parse_owner_repo(trimmed)
}

pub fn parse_owner_repo(value: &str) -> Option<String> {
    let mut parts = value.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() || !valid_repo_part(owner) || !valid_repo_part(repo) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !value.starts_with('.')
        && !value.ends_with('.')
}

pub fn valid_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value.chars().all(|c| c.is_ascii_hexdigit())
        && !value.chars().all(|c| c == '0')
}

pub fn version_is_newer(current: &str, latest: &str) -> bool {
    version_parts(latest) > version_parts(current)
}

pub fn version_parts(value: &str) -> (u64, u64, u64) {
    let mut parts = value
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

string_status_enum! {
    /// Lifecycle status of a single deployment, as persisted in the
    /// `deployments` table and sent over the wire.
    pub enum DeploymentStatus {
        Queued => "queued",
        Running => "running",
        Building => "building",
        Starting => "starting",
        HealthChecking => "health_checking",
        Routing => "routing",
        Success => "success",
        Failed => "failed",
        RolledBack => "rolled_back",
        Canceled => "canceled",
    }
}

#[cfg(test)]
mod contract_helper_tests {
    use super::*;

    #[test]
    fn app_slug_lowercases_and_replaces_non_alphanumeric() {
        assert_eq!(app_slug("My App"), "my-app");
        assert_eq!(app_slug("hello world"), "hello-world");
        assert_eq!(app_slug("UPPER"), "upper");
    }

    #[test]
    fn app_slug_trims_edge_hyphens() {
        assert_eq!(app_slug("-foo-"), "foo");
        assert_eq!(app_slug("--bar--"), "bar");
        assert_eq!(app_slug("  -baz-  "), "baz");
    }

    #[test]
    fn app_slug_falls_back_for_empty_and_all_junk_inputs() {
        assert_eq!(app_slug(""), "app");
        assert_eq!(app_slug("!!!"), "app");
        assert_eq!(app_slug("---"), "app");
        assert_eq!(app_slug("   "), "app");
    }

    #[test]
    fn clean_optional_command_preserves_three_way_distinction() {
        // Outer None → leave column untouched.
        assert_eq!(clean_optional_command(None), Ok(None));

        // Outer Some, inner None → clear the column.
        assert_eq!(clean_optional_command(Some(None)), Ok(Some(None)));

        // Outer Some, inner Some with a valid value → normalise and keep.
        assert_eq!(
            clean_optional_command(Some(Some("  npm run build  ".into()))),
            Ok(Some(Some("npm run build".into())))
        );

        // Outer Some, inner Some with an empty/whitespace value → collapse to clear.
        assert_eq!(
            clean_optional_command(Some(Some("   ".into()))),
            Ok(Some(None))
        );

        // Outer Some, inner Some with a value that fails clean_command validation.
        assert!(clean_optional_command(Some(Some("echo a\nrm -rf b".into()))).is_err());
    }

    #[test]
    fn app_slug_is_identity_for_uuid_derived_route_keys() {
        // Agent call sites always receive "app-{uuid}" — verify no edge-case
        // transformation occurs, keeping derived identities stable.
        assert_eq!(
            app_slug("app-550e8400-e29b-41d4-a716-446655440000"),
            "app-550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn github_repo_inputs_parse_to_owner_repo() {
        assert_eq!(
            parse_github_repo("https://github.com/go-gitea/gitea"),
            Some("go-gitea/gitea".into())
        );
        assert_eq!(
            parse_github_repo("git@github.com:owner/repo.git"),
            Some("owner/repo".into())
        );
        assert_eq!(parse_github_repo("owner/repo"), Some("owner/repo".into()));
        assert_eq!(parse_github_repo("https://example.com/owner/repo"), None);
    }

    #[test]
    fn container_names_are_limited_to_managed_hostlet_names() {
        assert!(valid_container_name("hostlet-app-123"));
        assert!(valid_container_name("hostlet-app_123.local"));
        assert!(!valid_container_name("other-app-123"));
        assert!(!valid_container_name("hostlet-app/../../bad"));
        assert!(!valid_container_name(&format!(
            "hostlet-{}",
            "a".repeat(140)
        )));
    }

    #[test]
    fn commit_sha_rejects_delete_marker() {
        assert!(!valid_commit_sha(
            "0000000000000000000000000000000000000000"
        ));
        assert!(valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
    }

    #[test]
    fn version_comparison_uses_numeric_triplets() {
        assert!(version_is_newer("v0.9.9", "v0.10.0"));
        assert!(!version_is_newer("v1.2.3", "v1.2.3"));
        assert_eq!(version_parts("v1.bad.3"), (1, 0, 3));
    }

    #[test]
    fn packaging_strategy_is_normalized() {
        assert_eq!(clean_packaging_strategy(None), Ok("auto".into()));
        assert_eq!(
            clean_packaging_strategy(Some(" generated ")),
            Ok("generated".into())
        );
        assert_eq!(
            clean_packaging_strategy(Some("compose")),
            Err("packaging strategy must be auto, dockerfile, or generated")
        );
    }

    #[test]
    fn dockerfile_inference_prefers_web_port_and_prompts_env() {
        let inference = infer_dockerfile(
            r#"
FROM alpine
ENV APP_SECRET=
ARG BUILD_TOKEN
EXPOSE 22 3000/tcp
VOLUME ["/data"]
"#,
        );

        assert_eq!(inference.port, Some(3000));
        assert!(inference.env.iter().any(|item| item["key"] == "APP_SECRET"));
        assert!(inference
            .warnings
            .iter()
            .any(|warning| warning.contains("multiple ports")));
        assert!(inference
            .warnings
            .iter()
            .any(|warning| warning.contains("BUILD_TOKEN")));
    }

    #[test]
    fn package_json_inference_detects_framework_and_manager() {
        let inference = infer_package_json(
            r#"{"dependencies":{"next":"16.0.0"},"packageManager":"pnpm@10.0.0"}"#,
            false,
            false,
            false,
        );

        assert_eq!(inference.framework, "Next.js");
        assert_eq!(inference.package_manager, "pnpm");
    }

    #[test]
    fn node_inspection_with_dockerfile_prefers_generated_packaging() {
        let inference = infer_package_json(
            r#"{"dependencies":{"next":"16.0.0"},"packageManager":"pnpm@10.0.0"}"#,
            false,
            false,
            false,
        );
        let value = node_inspection("owner/homebase", "main", "main", inference, true);

        assert_eq!(value["recommendedPackagingStrategy"], "generated");
        assert_eq!(
            value["packagingOptions"],
            serde_json::json!(["auto", "generated", "dockerfile"])
        );
    }

    #[test]
    fn inspection_payloads_emit_packaging_contract() {
        let value = railpack_inspection("owner/repo", "main", "main", "Python");

        assert_eq!(value["packagingStrategy"], "auto");
        assert_eq!(
            value["packagingOptions"],
            serde_json::json!(["auto", "generated"])
        );
        assert_eq!(value["recommendedPackagingStrategy"], "generated");
    }

    #[test]
    fn compose_inspection_emits_service_list_and_compose_runtime() {
        let services = compose::parse_compose_services(
            "services:\n  web:\n    build: .\n  redis:\n    image: redis:7-alpine\n",
            "web",
        );
        let compose = compose::HostletComposeSection {
            web_service: "web".to_string(),
            file: None,
            port: Some(3000),
            health_path: Some("/healthz".to_string()),
        };
        let value = compose_inspection(
            "owner/stack",
            "main",
            "main",
            "hostlet.yml",
            &compose,
            &services,
            &[],
        );

        assert_eq!(value["runtimeKind"], "compose");
        assert_eq!(value["webService"], "web");
        assert_eq!(value["deployable"], true);
        assert_eq!(value["healthPath"], "/healthz");
        assert_eq!(value["services"].as_array().unwrap().len(), 2);
        assert_eq!(value["services"][0]["role"], "web");
    }

    #[test]
    fn compose_inspection_with_subset_violation_is_not_deployable() {
        let services = compose::parse_compose_services("services:\n  web:\n    build: .\n", "web");
        let compose = compose::HostletComposeSection {
            web_service: "web".to_string(),
            file: None,
            port: None,
            health_path: None,
        };
        let value = compose_inspection(
            "owner/stack",
            "main",
            "main",
            "hostlet.yml",
            &compose,
            &services,
            &["Service web uses unsupported field ports".to_string()],
        );

        assert_eq!(value["deployable"], false);
        assert!(value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("ports")));
    }
}

string_status_enum! {
    /// Lifecycle status of a durable agent job, as persisted in the
    /// `agent_jobs` table and sent over the wire.
    ///
    /// Note both `Canceled` (`"canceled"`) and `Cancelled` (`"cancelled"`)
    /// exist on purpose: the agent-job pipeline persists the British spelling
    /// `"cancelled"` (see `web/jobs.rs` and `web/cleanup.rs`), while
    /// `"canceled"` is accepted/recognized for parity with [`DeploymentStatus`]
    /// and the agent status validator. They are distinct wire strings, so
    /// neither can be dropped without changing deserialization of existing rows.
    pub enum AgentJobStatus {
        Queued => "queued",
        Claimed => "claimed",
        Running => "running",
        Success => "success",
        Failed => "failed",
        Canceled => "canceled",
        Cancelled => "cancelled",
        Expired => "expired",
    }
}

string_status_enum! {
    /// Health classification reported for a running container.
    pub enum RuntimeHealthStatus {
        Healthy => "healthy",
        Degraded => "degraded",
        Unhealthy => "unhealthy",
        Unknown => "unknown",
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentJobPayload {
    Deploy(Box<DeployJob>),
    Rollback(Box<RollbackJob>),
    DeleteApp(Box<DeleteAppJob>),
    HealthCheck(Box<HealthCheckJob>),
    CaptureScreenshot(Box<CaptureScreenshotJob>),
    RestartContainer(Box<RestartContainerJob>),
    DockerCleanup(Box<DockerCleanupJob>),
}

/// Full work order handed to an agent to build and launch one deployment.
///
/// The fields fall into a few logical groups (kept in a flat struct so the JSON
/// wire shape is unchanged): identity/routing, source, networking, runtime
/// configuration, build/run commands, resource limits, and secrets.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeployJob {
    // Identity and routing.
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub route_key: String,
    pub app_name: String,

    // Source to build from.
    pub repo: String,
    pub branch: String,
    pub commit_sha: String,

    // Networking and health.
    pub container_port: i64,
    pub health_path: String,
    pub domain: String,

    // Runtime configuration.
    pub env: BTreeMap<String, String>,
    pub runtime_kind: String,
    pub hostlet_config_path: String,
    pub runtime_config: Value,
    pub packaging_strategy: String,
    pub root_directory: String,

    // Build and run commands (defaulted when absent).
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,

    // Resource limits.
    pub memory_limit_mb: Option<i32>,
    pub cpu_limit: Option<f64>,

    // Secrets.
    pub github_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RollbackJob {
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub route_key: String,
    pub target_deployment_id: Uuid,
    pub target_container: Option<String>,
    pub domain: String,
    pub container_port: i32,
    pub published_port: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeleteAppJob {
    pub app_id: Uuid,
    pub route_key: String,
    pub domain: String,
    pub user_id: Uuid,
    pub public_exposure: bool,
    pub compose_project: String,
    pub containers: Vec<String>,
    pub images: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HealthCheckJob {
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub container_name: String,
    pub published_port: i32,
    pub health_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CaptureScreenshotJob {
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub capture_url: String,
    pub width: i32,
    pub height: i32,
    pub format: String,
    pub screenshotter_image: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RestartContainerJob {
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub container_name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DockerCleanupJob {
    pub dry_run: bool,
    pub keep_containers: Vec<String>,
    pub keep_images: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Heartbeat,
    DeploymentStatus(DeploymentStatusEvent),
    Log(LogEvent),
    ResourceStats(ResourceStatsEvent),
    HealthStatus(HealthStatusEvent),
    JobStatus(JobStatusEvent),
    StorageStats(StorageStatsEvent),
}

/// The self-hosted default per-app managed-volume storage limit (MB). Hostlet
/// Cloud overrides this per plan via `runtime_config.compose.volumeStorageLimitMb`;
/// self-hosted apps fall back to this. Env-overridable by the API.
pub const DEFAULT_VOLUME_STORAGE_LIMIT_MB: i64 = 5120;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeploymentStatusEvent {
    pub deployment_id: Uuid,
    pub status: DeploymentStatus,
    pub failure: Option<String>,
    pub image_tag: Option<String>,
    pub container_name: Option<String>,
    pub local_url: Option<String>,
    pub published_port: Option<i32>,
    pub compose_project: Option<String>,
    pub runtime_metadata: Option<Value>,
    /// Per-service rows for a multi-service (Compose) deployment. Absent/empty
    /// for single-service apps. `#[serde(default)]` keeps the wire shape
    /// backward compatible with agents that predate multi-service reporting.
    #[serde(default)]
    pub services: Vec<DeploymentServiceReport>,
}

/// One Compose service as reported by the agent after `docker compose up`. The
/// API persists these into `deployment_services` and serves them back so the UI
/// can render a card per service.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentServiceReport {
    pub name: String,
    /// `"web"` for the routed entrypoint, `"backing"` for internal dependencies.
    pub role: String,
    pub container_name: Option<String>,
    pub image_tag: Option<String>,
    pub target_port: Option<i32>,
    pub published_port: Option<i32>,
    /// Container lifecycle state (e.g. `running`, `exited`).
    pub status: Option<String>,
    /// HTTP health classification; only meaningful for the web service.
    pub health_status: Option<String>,
}

/// One managed volume's measured disk usage, reported by the agent for the
/// per-service storage breakdown. `name` is the logical compose volume name
/// (e.g. `pgdata`) or the single-service data volume.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeUsage {
    pub name: String,
    pub used_bytes: i64,
}

/// Per-app managed-volume storage usage, sampled periodically by the agent
/// (`docker system df -v`) and persisted by the API for the quota meter + the
/// deploy gate. `used_bytes` is the combined size of all the app's managed
/// volumes (the web/data volume plus each add-on volume).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatsEvent {
    pub app_id: Uuid,
    pub used_bytes: i64,
    #[serde(default)]
    pub volumes: Vec<VolumeUsage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogEvent {
    pub deployment_id: Uuid,
    pub stream: String,
    pub line: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceStatsEvent {
    pub container: String,
    pub cpu_percent: String,
    pub cpu_percent_value: Option<f64>,
    pub memory_usage: String,
    pub memory_usage_bytes: Option<i64>,
    pub memory_limit_bytes: Option<i64>,
    pub memory_percent: String,
    pub memory_percent_value: Option<f64>,
    pub network_io: String,
    pub network_rx_bytes: Option<i64>,
    pub network_tx_bytes: Option<i64>,
    pub block_io: String,
    pub block_read_bytes: Option<i64>,
    pub block_write_bytes: Option<i64>,
    pub pids: String,
    pub pids_current: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HealthStatusEvent {
    pub app_id: Uuid,
    pub deployment_id: Option<Uuid>,
    pub container_name: Option<String>,
    pub status: RuntimeHealthStatus,
    pub checked_url: Option<String>,
    pub http_status: Option<i32>,
    pub latency_ms: Option<i32>,
    pub failure_count: i32,
    pub success_count: i32,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobStatusEvent {
    pub job_id: Uuid,
    pub status: AgentJobStatus,
    pub failure: Option<String>,
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
