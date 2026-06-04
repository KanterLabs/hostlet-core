use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Defines a `snake_case` status enum whose wire string is shared by serde, the
/// database, and the `as_str`/`FromStr` round trip.
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

/// Validates an HTTP health-check path.
///
/// Must be an absolute path (leading `/`), at most 256 chars, and free of
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
    fn inspection_payloads_emit_packaging_contract() {
        let value = railpack_inspection("owner/repo", "main", "main", "Python");

        assert_eq!(value["packagingStrategy"], "auto");
        assert_eq!(
            value["packagingOptions"],
            serde_json::json!(["auto", "generated"])
        );
        assert_eq!(value["recommendedPackagingStrategy"], "generated");
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

#[derive(Clone, Debug, PartialEq)]
pub struct DockerfileInference {
    pub port: Option<i32>,
    pub env: Vec<Value>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackageInference {
    pub framework: &'static str,
    pub package_manager: &'static str,
}

pub fn infer_package_json(
    contents: &str,
    has_bun_lock: bool,
    has_pnpm_lock: bool,
    has_yarn_lock: bool,
) -> PackageInference {
    let package: Value = serde_json::from_str(contents).unwrap_or_else(|_| serde_json::json!({}));
    let mut deps = std::collections::HashSet::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(map) = package.get(key).and_then(|value| value.as_object()) {
            deps.extend(map.keys().map(String::as_str));
        }
    }
    let framework = if deps.contains("next") {
        "Next.js"
    } else if deps.contains("astro") {
        "Astro"
    } else if deps.contains("nuxt") {
        "Nuxt"
    } else if deps.contains("@remix-run/node") || deps.contains("@remix-run/react") {
        "Remix"
    } else if deps.contains("@sveltejs/kit") {
        "SvelteKit"
    } else if deps.contains("vite") {
        "Vite"
    } else {
        "Node"
    };
    PackageInference {
        framework,
        package_manager: infer_package_manager(
            contents,
            has_bun_lock,
            has_pnpm_lock,
            has_yarn_lock,
        ),
    }
}

pub fn infer_package_manager(
    package_json: &str,
    has_bun_lock: bool,
    has_pnpm_lock: bool,
    has_yarn_lock: bool,
) -> &'static str {
    let package: Value =
        serde_json::from_str(package_json).unwrap_or_else(|_| serde_json::json!({}));
    let fallback_package_manager = if has_bun_lock {
        "bun"
    } else if has_pnpm_lock {
        "pnpm"
    } else if has_yarn_lock {
        "yarn"
    } else {
        "npm"
    };
    package
        .get("packageManager")
        .and_then(|value| value.as_str())
        .and_then(package_manager_from_field)
        .unwrap_or(fallback_package_manager)
}

fn package_manager_from_field(value: &str) -> Option<&'static str> {
    let manager = value.split('@').next().unwrap_or(value);
    match manager {
        "bun" => Some("bun"),
        "pnpm" => Some("pnpm"),
        "yarn" => Some("yarn"),
        "npm" => Some("npm"),
        _ => None,
    }
}

pub fn infer_dockerfile(contents: &str) -> DockerfileInference {
    let mut ports = Vec::new();
    let mut env = Vec::new();
    let mut warnings = vec![
        "Public Dockerfiles run arbitrary build steps on this machine. Review the upstream project before deploying.".to_string(),
    ];
    for line in contents.lines().map(str::trim) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EXPOSE ") {
            for token in line[7..].split_whitespace() {
                let port = token
                    .split('/')
                    .next()
                    .and_then(|part| part.parse::<i32>().ok());
                if let Some(port) = port {
                    ports.push(port);
                }
            }
        } else if upper.starts_with("ENV ") {
            for item in line[4..].split_whitespace() {
                let key = item.split('=').next().unwrap_or("").trim();
                if valid_env_prompt_key(key) {
                    env.push(serde_json::json!({"key": key, "required": false, "value": "", "source": "Dockerfile ENV"}));
                }
            }
        } else if upper.starts_with("ARG ") {
            let key = line[4..].split('=').next().unwrap_or("").trim();
            if valid_env_prompt_key(key) {
                warnings.push(format!("Dockerfile declares build arg {key}; Hostlet does not prompt for build args yet."));
            }
        } else if upper.starts_with("VOLUME ") {
            warnings.push("Dockerfile declares volumes. Hostlet provides /data automatically; verify the app persists data where expected.".into());
        }
    }
    ports.sort_unstable();
    ports.dedup();
    let preferred = [3000, 8080, 8000, 80, 5000, 4000]
        .into_iter()
        .find(|port| ports.contains(port))
        .or_else(|| ports.iter().copied().find(|port| *port != 22));
    if ports.len() > 1 {
        warnings.push(format!(
            "Dockerfile exposes multiple ports ({ports:?}); Hostlet selected {}.",
            preferred.unwrap_or(3000)
        ));
    }
    DockerfileInference {
        port: preferred,
        env,
        warnings,
    }
}

fn valid_env_prompt_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

pub fn dockerfile_inspection(
    repo: &str,
    branch: &str,
    default_branch: &str,
    inference: DockerfileInference,
) -> Value {
    inspection_base(InspectionBaseInput {
        repo,
        branch,
        default_branch,
        deployable: true,
        container_port: serde_json::json!(inference.port.unwrap_or(3000)),
        packaging_options: serde_json::json!(["auto", "dockerfile", "generated"]),
        recommended_packaging_strategy: "auto",
        env: serde_json::json!(inference.env),
        warnings: serde_json::json!(inference.warnings),
        summary: "Dockerfile detected. Hostlet inferred a single-container runtime.".to_string(),
    })
}

pub fn node_inspection(
    repo: &str,
    branch: &str,
    default_branch: &str,
    inference: PackageInference,
) -> Value {
    let mut result = object_map(inspection_base(InspectionBaseInput {
        repo,
        branch,
        default_branch,
        deployable: true,
        container_port: serde_json::json!(3000),
        packaging_options: serde_json::json!(["auto", "generated"]),
        recommended_packaging_strategy: "generated",
        env: serde_json::json!([]),
        warnings: serde_json::json!(["Node app detected. Hostlet will build it with Railpack unless a repository Dockerfile is selected. Set custom build/start commands if the preview is incomplete."]),
        summary: format!(
            "{} app detected. Hostlet will use generated Railpack runtime support with {}.",
            inference.framework, inference.package_manager
        ),
    }));
    result.insert(
        "detectedFramework".into(),
        serde_json::json!(inference.framework),
    );
    result.insert(
        "packageManager".into(),
        serde_json::json!(inference.package_manager),
    );
    Value::Object(result)
}

pub fn railpack_inspection(
    repo: &str,
    branch: &str,
    default_branch: &str,
    language: &str,
) -> Value {
    inspection_base(InspectionBaseInput {
        repo,
        branch,
        default_branch,
        deployable: true,
        container_port: serde_json::json!(3000),
        packaging_options: serde_json::json!(["auto", "generated"]),
        recommended_packaging_strategy: "generated",
        env: serde_json::json!([]),
        warnings: serde_json::json!([format!("{language} app detected. Hostlet will build it with Railpack if there is no repository Dockerfile.")]),
        summary: format!("{language} app detected. Hostlet will use generated Railpack runtime support."),
    })
}

pub fn unknown_inspection(repo: &str, branch: &str, default_branch: &str) -> Value {
    inspection_base(InspectionBaseInput {
        repo,
        branch,
        default_branch,
        deployable: false,
        container_port: serde_json::json!(3000),
        packaging_options: serde_json::json!(["auto"]),
        recommended_packaging_strategy: "auto",
        env: serde_json::json!([]),
        warnings: serde_json::json!(["No root Dockerfile, package.json, Python, Go, Rust, static, or Hostlet Compose marker was found. Add a start command or a supported app manifest before deploying."]),
        summary: "Hostlet could not infer a runnable app shape.".to_string(),
    })
}

const GITEA_GENERATED_COMPOSE: &str = "\
services:
  server:
    image: docker.gitea.com/gitea:latest-rootless
    restart: unless-stopped
    environment:
      GITEA__server__DOMAIN: localhost
      GITEA__server__HTTP_PORT: \"3000\"
      GITEA__database__DB_TYPE: sqlite3
    volumes:
      - gitea-data:/var/lib/gitea
      - gitea-config:/etc/gitea
volumes:
  gitea-data:
  gitea-config:
";

pub fn gitea_inspection(repo: &str, branch: &str, default_branch: &str) -> Value {
    serde_json::json!({
        "repoFullName": repo,
        "defaultBranch": default_branch,
        "branch": branch,
        "appName": "gitea",
        "deployable": true,
        "runtimeKind": "compose",
        "rootDirectory": ".",
        "containerPort": 3000,
        "healthPath": "/",
        "hostletConfigPath": "hostlet.yml",
        "runtimeConfig": {
            "generatedCompose": {
                "composeFile": "compose.generated.hostlet.yml",
                "webService": "server",
                "port": 3000,
                "healthPath": "/",
                "compose": GITEA_GENERATED_COMPOSE
            }
        },
        "packagingStrategy": "auto",
        "packagingOptions": ["auto"],
        "recommendedPackagingStrategy": "auto",
        "env": [],
        "warnings": ["Gitea SSH Git access is not exposed in Hostlet 0.3.9; use HTTPS Git through the web route.", "The generated Gitea default uses SQLite and named Docker volumes for the simplest self-hosted setup."],
        "summary": "Gitea detected. Hostlet will use the official rootless image with SQLite and persistent named volumes.",
        "autoDeployAvailable": false
    })
}

struct InspectionBaseInput<'a> {
    repo: &'a str,
    branch: &'a str,
    default_branch: &'a str,
    deployable: bool,
    container_port: Value,
    packaging_options: Value,
    recommended_packaging_strategy: &'a str,
    env: Value,
    warnings: Value,
    summary: String,
}

fn inspection_base(input: InspectionBaseInput<'_>) -> Value {
    serde_json::json!({
        "repoFullName": input.repo,
        "defaultBranch": input.default_branch,
        "branch": input.branch,
        "appName": input.repo.split('/').nth(1).unwrap_or("app"),
        "deployable": input.deployable,
        "runtimeKind": "single",
        "rootDirectory": ".",
        "containerPort": input.container_port,
        "healthPath": "/",
        "hostletConfigPath": "hostlet.yml",
        "runtimeConfig": {},
        "packagingStrategy": "auto",
        "packagingOptions": input.packaging_options,
        "recommendedPackagingStrategy": input.recommended_packaging_strategy,
        "env": input.env,
        "warnings": input.warnings,
        "summary": input.summary,
        "autoDeployAvailable": false
    })
}

fn object_map(value: Value) -> serde_json::Map<String, Value> {
    let Value::Object(map) = value else {
        unreachable!("inspection_base always returns an object")
    };
    map
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
}

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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogEvent {
    pub deployment_id: Uuid,
    pub stream: String,
    pub line: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceStatsEvent {
    pub container: String,
    pub cpu_percent: String,
    pub memory_usage: String,
    pub memory_percent: String,
    pub network_io: String,
    pub block_io: String,
    pub pids: String,
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
mod tests {
    use super::*;

    #[test]
    fn deploy_payload_keeps_existing_wire_shape() {
        let deployment_id = Uuid::nil();
        let app_id = Uuid::from_u128(1);
        let payload = AgentJobPayload::Deploy(Box::new(DeployJob {
            deployment_id,
            app_id,
            route_key: "app-1".into(),
            app_name: "demo".into(),
            repo: "owner/repo".into(),
            branch: "main".into(),
            commit_sha: "HEAD".into(),
            container_port: 3000,
            health_path: "/".into(),
            domain: "demo.example.test".into(),
            env: BTreeMap::new(),
            runtime_kind: "single".into(),
            hostlet_config_path: "hostlet.yml".into(),
            runtime_config: serde_json::json!({}),
            packaging_strategy: "auto".into(),
            root_directory: ".".into(),
            install_command: None,
            build_command: None,
            start_command: None,
            memory_limit_mb: Some(512),
            cpu_limit: Some(0.5),
            github_token: None,
        }));
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["type"], "deploy");
        assert_eq!(value["deployment_id"], deployment_id.to_string());
        assert_eq!(value["app_id"], app_id.to_string());
        assert_eq!(
            serde_json::from_value::<AgentJobPayload>(value).unwrap(),
            payload
        );
    }

    #[test]
    fn agent_events_keep_existing_type_tags() {
        let event = AgentEvent::Heartbeat;
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "heartbeat");
    }

    #[test]
    fn deployment_status_strings_match_database_values() {
        assert_eq!(DeploymentStatus::HealthChecking.as_str(), "health_checking");
        assert_eq!(DeploymentStatus::RolledBack.as_str(), "rolled_back");
        assert_eq!(
            "health_checking".parse::<DeploymentStatus>().unwrap(),
            DeploymentStatus::HealthChecking
        );
    }
}
