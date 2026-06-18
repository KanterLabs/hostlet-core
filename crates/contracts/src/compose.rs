//! Shared Docker Compose contract helpers used by repo inspection (preview) and
//! the agent (deploy-time enforcement).
//!
//! The forbidden-field policy lives here as the single source of truth so the
//! inspection preview ([`compose_subset_warnings`]) and the agent's enforcing
//! gate (`validate_compose_subset`) cannot drift apart.

use serde::{Deserialize, Serialize};

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
        let manifest: Self = serde_yaml::from_str(manifest_yaml).ok()?;
        (manifest.runtime == "compose").then_some(manifest)
    }

    /// The compose file the manifest points at, defaulting to `compose.yaml`.
    pub fn compose_file(&self) -> &str {
        self.compose.file.as_deref().unwrap_or("compose.yaml")
    }
}

/// Display-only summary of one Compose service, used to render the per-service
/// card stack in the UI.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSummary {
    pub name: String,
    /// `"web"` for the routed entrypoint, `"backing"` for internal dependencies.
    pub role: String,
    pub image: Option<String>,
    pub build: bool,
    pub ports: Vec<String>,
    pub volumes: Vec<String>,
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

/// A Compose volume source is a host bind (rather than a named volume) when it
/// looks like a path. Mirrors the agent's `is_host_bind_source`.
pub fn is_host_bind_source(value: &str) -> bool {
    value.starts_with('/') || value.starts_with('.') || value.contains('/') || value.contains('\\')
}

/// Parses a Compose file into display summaries, tagging `web_service` as the
/// web role. Returns an empty vec for unparseable YAML or a missing `services:`
/// block — callers treat that as "no preview available", not an error.
pub fn parse_compose_services(compose_yaml: &str, web_service: &str) -> Vec<ServiceSummary> {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(compose_yaml) else {
        return Vec::new();
    };
    let Some(services) = value.get("services").and_then(|v| v.as_mapping()) else {
        return Vec::new();
    };
    let mut summaries = Vec::new();
    for (name, service) in services {
        let Some(name) = name.as_str() else {
            continue;
        };
        let role = if name == web_service {
            "web"
        } else {
            "backing"
        };
        let mapping = service.as_mapping();
        let image = mapping
            .and_then(|m| map_get(m, "image"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let build = mapping.is_some_and(|m| map_has(m, "build"));
        let ports = mapping.map(|m| string_seq(m, "ports")).unwrap_or_default();
        let volumes = mapping
            .map(|m| string_seq(m, "volumes"))
            .unwrap_or_default();
        summaries.push(ServiceSummary {
            name: name.to_string(),
            role: role.to_string(),
            image,
            build,
            ports,
            volumes,
        });
    }
    summaries
}

/// Soft, non-failing mirror of the agent's `validate_compose_subset`. Returns a
/// human-readable warning for each thing the agent would reject at deploy time,
/// so inspection can warn before the user commits. An empty result means the
/// stack is within the safe subset.
pub fn compose_subset_warnings(compose_yaml: &str, web_service: &str) -> Vec<String> {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(compose_yaml) else {
        return vec!["Compose file is not valid YAML.".to_string()];
    };
    let mut warnings = Vec::new();
    if let Some(volumes) = value.get("volumes").and_then(|v| v.as_mapping()) {
        for (name, volume) in volumes {
            let name = name.as_str().unwrap_or("?");
            if let Some(mapping) = volume.as_mapping() {
                for field in FORBIDDEN_TOP_LEVEL_VOLUME_FIELDS {
                    if map_has(mapping, field) {
                        warnings.push(format!(
                            "Volume {name} uses unsupported field {field}; Hostlet only supports simple named volumes."
                        ));
                    }
                }
            }
        }
    }
    let Some(services) = value.get("services").and_then(|v| v.as_mapping()) else {
        warnings.push("Compose file defines no services.".to_string());
        return warnings;
    };
    let mut has_web = false;
    for (name, service) in services {
        let Some(name) = name.as_str() else {
            continue;
        };
        if name == web_service {
            has_web = true;
        }
        let Some(mapping) = service.as_mapping() else {
            continue;
        };
        for field in FORBIDDEN_SERVICE_FIELDS {
            if map_has(mapping, field) {
                warnings.push(format!(
                    "Service {name} uses unsupported field {field}; Hostlet will reject it at deploy. Remove it before deploying."
                ));
            }
        }
        for volume in string_seq(mapping, "volumes") {
            let source = volume.split(':').next().unwrap_or("");
            if is_host_bind_source(source) {
                warnings.push(format!(
                    "Service {name} uses a host bind mount ({volume}); only named volumes are allowed."
                ));
            }
        }
    }
    if !has_web {
        warnings.push(format!(
            "Declared web service {web_service} is not defined in the compose file."
        ));
    }
    warnings
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
    fn missing_web_service_warns() {
        let compose = "services:\n  api:\n    build: .\n";
        let warnings = compose_subset_warnings(compose, "web");
        assert!(warnings
            .iter()
            .any(|w| w.contains("web") && w.contains("not defined")));
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
    fn manifest_defaults_compose_file() {
        let manifest = HostletComposeManifest::parse_compose(
            "runtime: compose\ncompose:\n  web_service: web\n",
        )
        .unwrap();
        assert_eq!(manifest.compose_file(), "compose.yaml");
    }
}
