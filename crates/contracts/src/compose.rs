//! Shared Docker Compose contract helpers used by repo inspection (preview) and
//! the agent (deploy-time enforcement).
//!
//! The forbidden-field policy lives here as the single source of truth so the
//! inspection preview ([`compose_subset_warnings`]) and the agent's enforcing
//! gate (`validate_compose_subset`) cannot drift apart.

use serde::{Deserialize, Serialize};

#[path = "compose_addons.rs"]
mod compose_addons;
#[path = "compose_preview.rs"]
mod compose_preview;
pub use compose_addons::{
    add_on_catalog, generate_compose, resolve_managed_addons, AddOn, AddOnEnv, AddOnInject,
    GeneratedCompose, ResolvedAddons, WEB_IMAGE_ENV,
};
pub use compose_preview::{parse_compose_services, ServiceSummary};

/// Service-level Compose fields Hostlet refuses to run, because each breaches
/// the single-web-service + named-volumes safety model: host port exposure
/// (`ports`), host networking (`network_mode`/`networks`), privilege escalation
/// (`privileged`/`pid`/`ipc`), raw devices, or a fixed `container_name` that
/// would collide across tenants. The agent enforces this list at deploy time;
/// inspection surfaces it as soft warnings so the UI can flag it early.
pub const FORBIDDEN_SERVICE_FIELDS: &[&str] = &[
    "container_name",
    "network_mode",
    "privileged",
    "pid",
    "ipc",
    "devices",
    "networks",
    "ports",
];

/// Top-level `volumes:` fields that pull in host-backed or external storage
/// instead of a simple managed named volume.
pub const FORBIDDEN_TOP_LEVEL_VOLUME_FIELDS: &[&str] = &["driver", "driver_opts", "external"];

/// A repository's `hostlet.yml` Compose manifest — the bring-your-own-compose
/// entry point. The agent deploys from this; inspection reads it to preview the
/// stack. Mirrors the shape the agent resolves in `resolve_compose_manifest`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HostletComposeManifest {
    pub runtime: String,
    pub compose: HostletComposeSection,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HostletComposeSection {
    pub web_service: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub health_path: Option<String>,
}

impl HostletComposeManifest {
    /// Parses a `hostlet.yml` and returns it only when it declares a Compose
    /// runtime. Returns `None` for non-compose manifests or unparseable YAML so
    /// inspection can fall through to the next detector.
    pub fn parse_compose(manifest_yaml: &str) -> Option<Self> {
        Self::parse_compose_checked(manifest_yaml).ok().flatten()
    }

    /// Parses and validates an explicit Compose manifest for API inspection.
    ///
    /// `Ok(None)` means the file is valid YAML but does not declare the
    /// `compose` runtime, so normal single-service detectors may continue. An
    /// error means the file declares `runtime: compose` but does not satisfy
    /// the same shape/path constraints enforced by the deploy agent.
    pub fn parse_compose_checked(manifest_yaml: &str) -> Result<Option<Self>, String> {
        let value: serde_yaml::Value = serde_yaml::from_str(manifest_yaml)
            .map_err(|err| format!("hostlet manifest is not valid YAML: {err}"))?;
        let Some(runtime) = value.get("runtime") else {
            return Ok(None);
        };
        let Some(runtime) = runtime.as_str() else {
            return Err("hostlet manifest runtime must be a string".into());
        };
        if runtime != "compose" {
            return Ok(None);
        }
        let manifest: Self = serde_yaml::from_value(value)
            .map_err(|err| format!("hostlet Compose manifest has an invalid shape: {err}"))?;
        validate_compose_manifest(&manifest)?;
        Ok(Some(manifest))
    }

    /// The compose file the manifest points at, defaulting to `compose.yaml`.
    pub fn compose_file(&self) -> &str {
        self.compose.file.as_deref().unwrap_or("compose.yaml")
    }
}

/// Validates the manifest fields the deploy agent consumes before it reads the
/// referenced Compose file. Keep this in contracts so API inspection and agent
/// deployment reject the same explicit manifest values.
pub fn validate_compose_manifest(manifest: &HostletComposeManifest) -> Result<(), String> {
    if manifest.runtime != "compose" {
        return Err("hostlet manifest runtime must be compose".to_string());
    }
    validate_compose_service_name(&manifest.compose.web_service).map_err(|err| err.to_string())?;
    if !crate::valid_relative_file_path(manifest.compose_file()) {
        return Err("compose file path must be a relative file path inside the repository".into());
    }
    if manifest.compose.port == Some(0) {
        return Err("compose port must be from 1 to 65535".into());
    }
    if let Some(path) = manifest.compose.health_path.as_deref() {
        if !crate::valid_health_path(path) {
            return Err("compose health path is invalid".into());
        }
    }
    Ok(())
}

/// The service-name grammar used when a Compose service is passed to Docker
/// command arguments. In particular, uppercase names and underscores are not
/// accepted by the agent.
pub fn validate_compose_service_name(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 48
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err("compose service names must use lowercase letters, numbers, and hyphens");
    }
    Ok(())
}

/// Top-level Compose volume names use the same Docker-safe grammar as service
/// names in the agent's release override (`lowercase`, digits, and hyphens).
pub fn validate_compose_volume_name(value: &str) -> Result<(), &'static str> {
    validate_compose_service_name(value)
}

fn map_get<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    mapping.get(serde_yaml::Value::String(key.to_string()))
}

fn map_has(mapping: &serde_yaml::Mapping, key: &str) -> bool {
    mapping.contains_key(serde_yaml::Value::String(key.to_string()))
}

/// Collects the string entries of a service's `key:` sequence (e.g. `ports`,
/// `volumes`). Long-form mapping entries are skipped — the preview only needs
/// the human-readable short forms.
fn string_seq(mapping: &serde_yaml::Mapping, key: &str) -> Vec<String> {
    map_get(mapping, key)
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The Docker control socket path. Mounting it into any container hands over
/// full host control, so both the preview and the agent reject it as a volume
/// target regardless of how the mount is expressed.
const DOCKER_SOCKET_PATH: &str = "/var/run/docker.sock";

/// A Compose volume source is a host bind (rather than a named volume) when it
/// looks like a path. Mirrors the agent's `is_host_bind_source`.
pub fn is_host_bind_source(value: &str) -> bool {
    value.starts_with('/') || value.starts_with('.') || value.contains('/') || value.contains('\\')
}

/// A relative, within-repo host bind (`./data`, `cache/x`) the agent auto-maps
/// onto a managed named volume at deploy time. Mirrors the agent's
/// `is_mappable_relative_bind`, so the inspection preview does not flag something
/// the agent will silently handle. Absolute and `..`-escaping paths are excluded.
pub fn is_mappable_relative_bind(source: &str) -> bool {
    is_host_bind_source(source)
        && !source.starts_with('/')
        && !source.split('/').any(|part| part == "..")
}

/// Validates an absolute container mount path (e.g. `/app/data`): absolute, at
/// most 256 chars, not the bare root `/`, no `..` segment, and free of control
/// characters, backslashes, and Docker `--mount` option delimiters. Used to vet
/// the data-volume mount path before the agent mounts a managed volume there.
pub fn valid_container_mount_path(value: &str) -> bool {
    value.starts_with('/')
        && value != "/"
        && value.len() <= 256
        && !value.split('/').any(|part| part == "..")
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ',' | '='))
}

/// Detects the container path an app declares for persistent data, from the
/// first relative host-bind in its compose (e.g. `./data:/app/data` → `/app/data`).
/// Cloud deploys such apps single-service (dropping the compose), so this lets the
/// single-service managed volume mount where the app actually writes instead of
/// the default `/data`. Returns `None` when no valid declared path is found.
pub fn detect_data_mount_path(compose_yaml: &str) -> Option<String> {
    parse_compose_services(compose_yaml, "")
        .iter()
        .flat_map(|service| service.volumes.iter())
        .find_map(|volume| {
            let mut parts = volume.splitn(3, ':');
            let source = parts.next()?;
            let target = parts.next()?;
            (is_mappable_relative_bind(source) && valid_container_mount_path(target))
                .then(|| target.to_string())
        })
}

/// Merges `runtimeConfig.dataMountPath` onto an inspection payload (preserving any
/// existing runtimeConfig), so the create handler stores it and the agent mounts
/// the single-service managed volume at this path.
pub fn with_data_mount_path(mut inspection: serde_json::Value, path: &str) -> serde_json::Value {
    let Some(map) = inspection.as_object_mut() else {
        return inspection;
    };
    let runtime_config = map
        .entry("runtimeConfig")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(rc) = runtime_config.as_object_mut() {
        rc.insert("dataMountPath".to_string(), serde_json::json!(path));
    }
    inspection
}

/// Returns the warning tail (everything after the `Service {name} ` prefix) for a
/// single `volumes:` entry the agent's `validate_compose_subset` would reject at
/// deploy time, or `None` when the entry is within the safe named-volume subset.
///
/// Handles both the short-form `source:target` string and the long-form
/// `{type, source, target}` mapping the earlier preview skipped, so the preview
/// flags exactly what the agent blocks. Short-form relative, within-repo binds
/// return `None` because the agent auto-maps them onto a managed volume before
/// validating; long-form entries are never auto-mapped, so any host-backed
/// source — relative or absolute — is flagged.
fn volume_subset_warning_for(
    volume: &serde_yaml::Value,
    allow_mappable_relative_binds: bool,
) -> Option<String> {
    if let Some(text) = volume.as_str() {
        let mut parts = text.split(':');
        let source = parts.next().unwrap_or("");
        if is_host_bind_source(source)
            && (!allow_mappable_relative_binds || !is_mappable_relative_bind(source))
        {
            return Some(format!(
                "uses a host bind mount ({text}); only named volumes are allowed."
            ));
        }
        if matches!(parts.next(), Some(target) if target == DOCKER_SOCKET_PATH) {
            return Some(format!(
                "mounts the Docker socket ({text}); Hostlet will reject it at deploy."
            ));
        }
        return None;
    }
    let mapping = volume.as_mapping()?;
    let volume_type = map_get(mapping, "type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let source = map_get(mapping, "source")
        .or_else(|| map_get(mapping, "src"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if volume_type == "bind" || is_host_bind_source(source) {
        let detail = if source.is_empty() {
            "type: bind".to_string()
        } else {
            format!("source: {source}")
        };
        return Some(format!(
            "uses a host bind mount ({detail}); only named volumes are allowed."
        ));
    }
    let target = map_get(mapping, "target")
        .or_else(|| map_get(mapping, "dst"))
        .or_else(|| map_get(mapping, "destination"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if target == DOCKER_SOCKET_PATH {
        return Some("mounts the Docker socket; Hostlet will reject it at deploy.".to_string());
    }
    None
}

/// Validates the YAML shape of one service-level volume entry before applying
/// the safety-subset checks. Docker Compose requires the field to be a
/// sequence, with each entry represented as short-form text or a long-form
/// object containing a target path; keeping that shape check here prevents the
/// API preview and agent from deferring malformed input to Docker.
fn short_volume_target_shape_error(text: &str) -> Option<&'static str> {
    let mut parts = text.split(':');
    let source = parts.next().unwrap_or_default();
    let target = parts.next();
    let _mode = parts.next();
    if parts.next().is_some() {
        return Some("volume short-form entries must have at most source, target, and mode.");
    }
    let has_source = target.is_some();
    let target = target.unwrap_or(source);
    if target.is_empty() {
        return Some("volume entries must define a non-empty target.");
    }
    // A colon means the first component is a source, so an empty source is
    // malformed (`:/data`) even when the target itself would be absolute.
    if has_source && source.is_empty() {
        return Some("volume entries must define a non-empty source.");
    }
    if target.starts_with('/') {
        return None;
    }
    Some("volume targets must be absolute container paths.")
}

fn volume_entry_shape_error(volume: &serde_yaml::Value) -> Option<&'static str> {
    if let Some(text) = volume.as_str() {
        return short_volume_target_shape_error(text);
    }
    let Some(mapping) = volume.as_mapping() else {
        return Some("volume entries must be strings or objects.");
    };
    for field in ["type", "source", "src", "target", "dst", "destination"] {
        if map_get(mapping, field).is_some_and(|value| !value.is_string()) {
            return Some("volume object fields must be strings.");
        }
    }
    let Some(target) = map_get(mapping, "target")
        .or_else(|| map_get(mapping, "dst"))
        .or_else(|| map_get(mapping, "destination"))
        .and_then(serde_yaml::Value::as_str)
    else {
        return Some("volume objects must define a non-empty target.");
    };
    if target.is_empty() {
        return Some("volume objects must define a non-empty target.");
    }
    if !target.starts_with('/') {
        return Some("volume targets must be absolute container paths.");
    }
    None
}

/// Returns every structural and safe-subset violation in a Compose file.
///
/// The agent invokes this with `allow_mappable_relative_binds = false` after
/// it has remapped short-form relative binds to managed named volumes. API
/// inspection invokes it with `true` against the source file, matching the
/// agent's pre-validation remap behavior without maintaining a second parser.
pub fn compose_subset_errors(
    compose_yaml: &str,
    web_service: &str,
    allow_mappable_relative_binds: bool,
) -> Vec<String> {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(compose_yaml) else {
        return vec!["Compose file is not valid YAML.".to_string()];
    };
    let mut errors = Vec::new();
    if validate_compose_service_name(web_service).is_err() {
        errors.push(format!(
            "Declared web service {web_service} has an invalid Compose service name."
        ));
    }

    match value.get("volumes") {
        None => {}
        Some(volumes) => {
            let Some(volumes) = volumes.as_mapping() else {
                errors.push("Compose top-level volumes must be a mapping.".to_string());
                // The agent cannot inspect entries from a non-mapping value.
                // Continue with services so inspection can surface all useful
                // violations in one response.
                return compose_subset_errors_with_services(
                    &value,
                    web_service,
                    allow_mappable_relative_binds,
                    errors,
                );
            };
            for (name, volume) in volumes {
                let Some(name) = name.as_str() else {
                    errors.push("Compose volume names must be strings.".to_string());
                    continue;
                };
                if validate_compose_volume_name(name).is_err() {
                    errors.push(format!(
                        "Compose volume names must use lowercase letters, numbers, and hyphens (invalid volume {name})."
                    ));
                }
                match volume {
                    serde_yaml::Value::Null => {}
                    serde_yaml::Value::Mapping(mapping) => {
                        for field in FORBIDDEN_TOP_LEVEL_VOLUME_FIELDS {
                            if map_has(mapping, field) {
                                errors.push(format!(
                                    "Volume {name} uses unsupported field {field}; Hostlet only supports simple named volumes."
                                ));
                            }
                        }
                    }
                    _ => errors.push(format!("Compose volume {name} must be an object.")),
                }
            }
        }
    }
    compose_subset_errors_with_services(&value, web_service, allow_mappable_relative_binds, errors)
}

fn compose_subset_errors_with_services(
    value: &serde_yaml::Value,
    web_service: &str,
    allow_mappable_relative_binds: bool,
    mut errors: Vec<String>,
) -> Vec<String> {
    let Some(services) = value.get("services").and_then(|v| v.as_mapping()) else {
        errors.push("Compose file must define services.".to_string());
        return errors;
    };
    let mut has_web = false;
    for (name, service) in services {
        let Some(name) = name.as_str() else {
            errors.push("Compose service names must be strings.".to_string());
            continue;
        };
        if name == web_service {
            has_web = true;
        }
        if validate_compose_service_name(name).is_err() {
            errors.push(format!(
                "Compose service names must use lowercase letters, numbers, and hyphens (invalid service {name})."
            ));
        }
        let Some(mapping) = service.as_mapping() else {
            errors.push(format!("Compose service {name} must be an object."));
            continue;
        };
        for field in FORBIDDEN_SERVICE_FIELDS {
            if map_has(mapping, field) {
                errors.push(format!(
                    "Service {name} uses unsupported field {field}; Hostlet will reject it at deploy. Remove it before deploying."
                ));
            }
        }
        if let Some(volumes) = map_get(mapping, "volumes") {
            let Some(volumes) = volumes.as_sequence() else {
                errors.push(format!("Service {name} volumes must be a sequence."));
                continue;
            };
            for volume in volumes {
                if let Some(shape_error) = volume_entry_shape_error(volume) {
                    errors.push(format!("Service {name} {shape_error}"));
                    continue;
                }
                if let Some(tail) = volume_subset_warning_for(volume, allow_mappable_relative_binds)
                {
                    errors.push(format!("Service {name} {tail}"));
                }
            }
        }
    }
    if !has_web {
        errors.push(format!(
            "Declared web service {web_service} is not defined in the compose file."
        ));
    }
    errors
}

/// Enforcing counterpart to [`compose_subset_warnings`]. The error text is
/// intentionally aggregated so API inspection can expose all violations while
/// the agent can return one context-rich `anyhow` error.
pub fn validate_compose_subset(compose_yaml: &str, web_service: &str) -> Result<(), String> {
    let errors = compose_subset_errors(compose_yaml, web_service, false);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Soft, non-failing mirror of the agent's `validate_compose_subset`. Returns a
/// human-readable warning for each thing the agent would reject at deploy time,
/// so inspection can warn before the user commits. An empty result means the
/// stack is within the safe subset.
pub fn compose_subset_warnings(compose_yaml: &str, web_service: &str) -> Vec<String> {
    compose_subset_errors(compose_yaml, web_service, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFE_COMPOSE: &str = "\
services:
  web:
    build: .
    volumes:
      - app-data:/data
  redis:
    image: redis:7-alpine
    volumes:
      - redis-data:/data
volumes:
  app-data:
  redis-data:
";

    #[test]
    fn parse_services_tags_web_role_and_reads_image_build() {
        let services = parse_compose_services(SAFE_COMPOSE, "web");
        assert_eq!(services.len(), 2);
        let web = services.iter().find(|s| s.name == "web").unwrap();
        assert_eq!(web.role, "web");
        assert!(web.build);
        assert_eq!(web.image, None);
        let redis = services.iter().find(|s| s.name == "redis").unwrap();
        assert_eq!(redis.role, "backing");
        assert!(!redis.build);
        assert_eq!(redis.image.as_deref(), Some("redis:7-alpine"));
        assert_eq!(redis.volumes, vec!["redis-data:/data".to_string()]);
    }

    #[test]
    fn safe_compose_has_no_subset_warnings() {
        assert!(compose_subset_warnings(SAFE_COMPOSE, "web").is_empty());
    }

    #[test]
    fn forbidden_fields_and_bind_mounts_warn() {
        let compose = "\
services:
  web:
    build: .
    ports:
      - 8080:80
  db:
    image: postgres:16
    privileged: true
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
";
        let warnings = compose_subset_warnings(compose, "web");
        assert!(warnings
            .iter()
            .any(|w| w.contains("web") && w.contains("ports")));
        assert!(warnings
            .iter()
            .any(|w| w.contains("db") && w.contains("privileged")));
        assert!(warnings.iter().any(|w| w.contains("host bind mount")));
    }

    #[test]
    fn relative_host_bind_is_auto_mapped_not_a_blocking_warning() {
        // Mirrors homebase: a single web service persisting to ./data. The agent
        // auto-maps this to a managed volume, so the preview must not flag it
        // (which would render the app undeployable).
        let compose = "services:\n  web:\n    build: .\n    volumes:\n      - ./data:/app/data\n";
        assert!(compose_subset_warnings(compose, "web").is_empty());
        // Absolute and escaping binds are still blocking.
        let absolute = "services:\n  web:\n    build: .\n    volumes:\n      - /etc:/host-etc\n";
        assert!(compose_subset_warnings(absolute, "web")
            .iter()
            .any(|w| w.contains("host bind mount")));
    }

    #[test]
    fn preview_warns_on_long_form_absolute_bind() {
        // The long-form `{type: bind, source, target}` object the preview used to
        // skip. The agent rejects it at deploy, so the preview must warn too.
        let bind_type = "services:\n  web:\n    build: .\n    volumes:\n      - type: bind\n        source: /etc\n        target: /host\n";
        assert!(compose_subset_warnings(bind_type, "web")
            .iter()
            .any(|w| w.contains("web") && w.contains("host bind mount")));
        // Even without an explicit `type: bind`, an absolute source is host-backed.
        let absolute_source = "services:\n  web:\n    build: .\n    volumes:\n      - source: /etc\n        target: /host\n";
        assert!(compose_subset_warnings(absolute_source, "web")
            .iter()
            .any(|w| w.contains("host bind mount")));
    }

    #[test]
    fn preview_warns_on_long_form_relative_bind_the_agent_rejects() {
        // Long-form entries are never auto-mapped (only short-form strings are),
        // so a relative host-backed source is still blocking — mirror that.
        let compose = "services:\n  web:\n    build: .\n    volumes:\n      - type: volume\n        source: data/cache\n        target: /app/data\n";
        assert!(compose_subset_warnings(compose, "web")
            .iter()
            .any(|w| w.contains("host bind mount")));
    }

    #[test]
    fn preview_warns_on_long_form_docker_socket_target() {
        // A named source but a socket target is rejected by the agent even though
        // the source itself is safe.
        let compose = "services:\n  web:\n    build: .\n    volumes:\n      - type: volume\n        source: docker-sock\n        target: /var/run/docker.sock\nvolumes:\n  docker-sock:\n";
        assert!(compose_subset_warnings(compose, "web")
            .iter()
            .any(|w| w.contains("Docker socket")));
    }

    #[test]
    fn preview_allows_string_form_relative_bind_and_long_form_named_volume() {
        // Short-form relative, within-repo bind: the agent auto-maps it, so the
        // preview must not flag it (that would make the app undeployable).
        let string_relative =
            "services:\n  web:\n    build: .\n    volumes:\n      - ./data:/app/data\n";
        assert!(compose_subset_warnings(string_relative, "web").is_empty());
        // A long-form named volume is the accepted subset — also no warning.
        let long_named = "services:\n  web:\n    build: .\n    volumes:\n      - type: volume\n        source: app-data\n        target: /data\nvolumes:\n  app-data:\n";
        assert!(compose_subset_warnings(long_named, "web").is_empty());
    }

    #[test]
    fn detects_declared_data_mount_path_from_relative_bind() {
        let compose = "services:\n  web:\n    build: .\n    volumes:\n      - ./data:/app/data\n";
        assert_eq!(
            detect_data_mount_path(compose).as_deref(),
            Some("/app/data")
        );
        // A named volume or an absolute bind is not a declared app data path.
        assert_eq!(
            detect_data_mount_path("services:\n  web:\n    volumes:\n      - app-data:/data\n"),
            None
        );
        assert_eq!(
            detect_data_mount_path("services:\n  web:\n    volumes:\n      - /etc:/host-etc\n"),
            None
        );
        assert_eq!(
            detect_data_mount_path("services:\n  web:\n    build: .\n"),
            None
        );
    }

    #[test]
    fn container_mount_path_validation() {
        assert!(valid_container_mount_path("/app/data"));
        assert!(valid_container_mount_path("/data"));
        assert!(!valid_container_mount_path("/"));
        assert!(!valid_container_mount_path("app/data"));
        assert!(!valid_container_mount_path("/app/../etc"));
        assert!(!valid_container_mount_path("/app\\data"));
        assert!(!valid_container_mount_path("/host,type=bind,source=/"));
        assert!(!valid_container_mount_path("/app/data=prod"));
    }

    #[test]
    fn with_data_mount_path_merges_into_runtime_config() {
        let inspection = serde_json::json!({"runtimeKind": "single", "runtimeConfig": {"foo": 1}});
        let out = with_data_mount_path(inspection, "/app/data");
        assert_eq!(
            out.pointer("/runtimeConfig/dataMountPath").unwrap(),
            "/app/data"
        );
        assert_eq!(out.pointer("/runtimeConfig/foo").unwrap(), 1);
    }

    #[test]
    fn missing_web_service_warns() {
        let compose = "services:\n  api:\n    build: .\n";
        let warnings = compose_subset_warnings(compose, "web");
        assert!(warnings
            .iter()
            .any(|w| w.contains("web") && w.contains("not defined")));
    }

    #[test]
    fn preview_rejects_uppercase_and_non_object_services() {
        let uppercase = "services:\n  Web:\n    image: app\n";
        let warnings = compose_subset_warnings(uppercase, "Web");
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("invalid service Web")));

        let null_service = "services:\n  web: null\n";
        let warnings = compose_subset_warnings(null_service, "web");
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("must be an object")));
        assert!(validate_compose_subset(null_service, "web").is_err());
    }

    #[test]
    fn preview_rejects_non_mapping_top_level_volumes() {
        let warnings =
            compose_subset_warnings("services:\n  web:\n    image: app\nvolumes: []\n", "web");
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("top-level volumes must be a mapping")));
        assert!(
            validate_compose_subset("services:\n  web:\n    image: app\nvolumes: []\n", "web")
                .is_err()
        );

        let invalid_name = "services:\n  web:\n    image: app\nvolumes:\n  Cache_Data:\n";
        assert!(compose_subset_warnings(invalid_name, "web")
            .iter()
            .any(|warning| warning.contains("invalid volume Cache_Data")));
        assert!(validate_compose_subset(invalid_name, "web").is_err());
    }

    #[test]
    fn preview_rejects_malformed_service_volume_shapes() {
        let mapping = "services:\n  web:\n    image: app\n    volumes: {}\n";
        assert!(compose_subset_warnings(mapping, "web")
            .iter()
            .any(|warning| warning.contains("volumes must be a sequence")));
        assert!(validate_compose_subset(mapping, "web").is_err());

        let null_entry = "services:\n  web:\n    image: app\n    volumes:\n      - null\n";
        assert!(compose_subset_warnings(null_entry, "web")
            .iter()
            .any(|warning| warning.contains("strings or objects")));
        assert!(validate_compose_subset(null_entry, "web").is_err());

        let missing_target = "services:\n  web:\n    image: app\n    volumes:\n      - type: volume\n        source: app-data\n";
        assert!(compose_subset_warnings(missing_target, "web")
            .iter()
            .any(|warning| warning.contains("non-empty target")));
        assert!(validate_compose_subset(missing_target, "web").is_err());

        for entry in ["", "app-data", "app-data:", "app-data:relative"] {
            let yaml_entry = serde_yaml::to_string(entry).unwrap();
            let compose =
                format!("services:\n  web:\n    image: app\n    volumes:\n      - {yaml_entry}");
            assert!(compose_subset_warnings(&compose, "web")
                .iter()
                .any(|warning| warning.contains("target") || warning.contains("absolute")));
            assert!(validate_compose_subset(&compose, "web").is_err());
        }

        let relative_long_target = "services:\n  web:\n    image: app\n    volumes:\n      - type: volume\n        source: app-data\n        target: data\n";
        assert!(compose_subset_warnings(relative_long_target, "web")
            .iter()
            .any(|warning| warning.contains("absolute")));
        assert!(validate_compose_subset(relative_long_target, "web").is_err());

        let valid_short_target = "services:\n  web:\n    image: app\n    volumes:\n      - app-data:/data\nvolumes:\n  app-data:\n";
        assert!(compose_subset_warnings(valid_short_target, "web").is_empty());
        let valid_long_target = "services:\n  web:\n    image: app\n    volumes:\n      - type: volume\n        source: app-data\n        target: /data\nvolumes:\n  app-data:\n";
        assert!(compose_subset_warnings(valid_long_target, "web").is_empty());
    }

    #[test]
    fn invalid_yaml_is_a_single_warning_not_a_panic() {
        let warnings = compose_subset_warnings("::: not yaml :::", "web");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not valid YAML"));
        assert!(parse_compose_services("::: not yaml :::", "web").is_empty());
    }

    #[test]
    fn manifest_parses_only_compose_runtime() {
        let manifest = HostletComposeManifest::parse_compose(
            "runtime: compose\ncompose:\n  web_service: server\n  file: stack.yml\n  port: 8080\n",
        )
        .unwrap();
        assert_eq!(manifest.compose.web_service, "server");
        assert_eq!(manifest.compose_file(), "stack.yml");
        assert_eq!(manifest.compose.port, Some(8080));
        assert!(HostletComposeManifest::parse_compose("runtime: single\n").is_none());
        assert!(HostletComposeManifest::parse_compose(":::bad").is_none());
    }

    #[test]
    fn checked_manifest_rejects_invalid_service_and_repeated_slash_path() {
        let uppercase = HostletComposeManifest::parse_compose_checked(
            "runtime: compose\ncompose:\n  web_service: Web\n",
        );
        assert!(uppercase.is_err());
        let repeated_slash = HostletComposeManifest::parse_compose_checked(
            "runtime: compose\ncompose:\n  web_service: web\n  file: config//compose.yml\n",
        );
        assert!(repeated_slash.is_err());
        for file in [
            "/compose.yml",
            "../compose.yml",
            "compose\\windows.yml",
            "config/compose?.yml",
            "config/compose#.yml",
            "",
        ] {
            let yaml_file = serde_yaml::to_string(file).unwrap();
            let manifest =
                format!("runtime: compose\ncompose:\n  web_service: web\n  file: {yaml_file}");
            assert!(
                HostletComposeManifest::parse_compose_checked(&manifest).is_err(),
                "manifest path {file:?} must be rejected"
            );
        }
        assert!(HostletComposeManifest::parse_compose_checked("runtime: 1\n").is_err());
    }

    #[test]
    fn manifest_defaults_compose_file() {
        let manifest = HostletComposeManifest::parse_compose(
            "runtime: compose\ncompose:\n  web_service: web\n",
        )
        .unwrap();
        assert_eq!(manifest.compose_file(), "compose.yaml");
    }
}
