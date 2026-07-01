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
fn volume_subset_warning(volume: &serde_yaml::Value) -> Option<String> {
    if let Some(text) = volume.as_str() {
        let mut parts = text.split(':');
        let source = parts.next().unwrap_or("");
        if is_host_bind_source(source) && !is_mappable_relative_bind(source) {
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
        if let Some(volumes) = map_get(mapping, "volumes").and_then(|v| v.as_sequence()) {
            // Covers both short-form strings and the long-form `{type, source,
            // target}` mappings; relative, within-repo string binds are auto-mapped
            // by the agent so they intentionally produce no warning.
            for volume in volumes {
                if let Some(tail) = volume_subset_warning(volume) {
                    warnings.push(format!("Service {name} {tail}"));
                }
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

/// The compose-interpolated environment variable the agent sets to the freshly
/// built web image at deploy time, so the generated stack references the user's
/// Railpack/Dockerfile-built app without that image ref being known at create
/// time. Backing-service secrets are interpolated the same way (`${KEY}`), with
/// values sourced from the app's encrypted env — never stored in the generated
/// compose itself.
pub const WEB_IMAGE_ENV: &str = "HOSTLET_WEB_IMAGE";

/// One environment variable a managed add-on's container needs. `generate`
/// secrets are minted per app at create time; the rest fall back to `default`.
/// Either way the value is stored in the app's encrypted env and reaches the
/// container via `${KEY}` interpolation — it is never embedded in the compose.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOnEnv {
    pub key: String,
    pub generate: bool,
    pub default: Option<String>,
}

/// A connection value injected into the *web* service (e.g. `DATABASE_URL`),
/// rendered from the add-on's env at create time and stored encrypted.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOnInject {
    pub key: String,
    pub template: String,
}

/// A managed backing-service add-on from the built-in catalog. Generic and
/// self-hostable: `min_plan` is data the cloud layer enforces as policy; core
/// ignores it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOn {
    pub key: String,
    pub name: String,
    pub category: String,
    pub icon: String,
    pub image: String,
    pub service_name: String,
    pub port: u16,
    pub volumes: Vec<String>,
    pub env: Vec<AddOnEnv>,
    pub inject: Vec<AddOnInject>,
    pub min_plan: String,
}

fn env_var(key: &str, generate: bool, default: Option<&str>) -> AddOnEnv {
    AddOnEnv {
        key: key.to_string(),
        generate,
        default: default.map(str::to_string),
    }
}

/// The built-in managed add-on catalog. Start small (Postgres, Redis); each
/// entry's image is vetted to run under Hostlet's per-service hardening (no host
/// ports, default caps, `no-new-privileges`).
pub fn add_on_catalog() -> Vec<AddOn> {
    vec![
        AddOn {
            key: "postgres".to_string(),
            name: "PostgreSQL".to_string(),
            category: "database".to_string(),
            icon: "database".to_string(),
            image: "postgres:16-alpine".to_string(),
            service_name: "postgres".to_string(),
            port: 5432,
            volumes: vec!["pgdata:/var/lib/postgresql/data".to_string()],
            env: vec![
                env_var("POSTGRES_PASSWORD", true, None),
                env_var("POSTGRES_USER", false, Some("postgres")),
                env_var("POSTGRES_DB", false, Some("app")),
            ],
            inject: vec![AddOnInject {
                key: "DATABASE_URL".to_string(),
                template:
                    "postgres://${POSTGRES_USER}:${POSTGRES_PASSWORD}@postgres:5432/${POSTGRES_DB}"
                        .to_string(),
            }],
            min_plan: "starter".to_string(),
        },
        AddOn {
            key: "redis".to_string(),
            name: "Redis".to_string(),
            category: "cache".to_string(),
            icon: "zap".to_string(),
            image: "redis:7-alpine".to_string(),
            service_name: "redis".to_string(),
            port: 6379,
            volumes: vec!["redis-data:/data".to_string()],
            env: vec![],
            inject: vec![AddOnInject {
                key: "REDIS_URL".to_string(),
                template: "redis://redis:6379".to_string(),
            }],
            min_plan: "starter".to_string(),
        },
    ]
}

/// The generated Compose runtime for a managed-add-ons app, matching the
/// `generatedCompose` shape the agent already consumes in
/// `resolve_compose_manifest` (composeFile/webService/port/healthPath/compose).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedCompose {
    pub compose_file: String,
    pub web_service: String,
    pub port: u16,
    pub health_path: String,
    pub compose: String,
}

/// Generates the Compose YAML for a web app plus managed add-ons.
///
/// The web service references the agent-built image via `${HOSTLET_WEB_IMAGE}`
/// (so the built image ref need not be known at create time); each add-on
/// service references its env via `${KEY}` interpolation. No service declares a
/// host `ports:` mapping — the web service's loopback publish is added by the
/// agent's compose override — so the output is always within the safe subset.
pub fn generate_compose(
    web_service: &str,
    port: u16,
    health_path: &str,
    addons: &[AddOn],
) -> GeneratedCompose {
    use serde_yaml::{Mapping, Value};

    let mut services = Mapping::new();
    let mut web = Mapping::new();
    web.insert(
        Value::from("image"),
        Value::from(format!("${{{WEB_IMAGE_ENV}}}")),
    );
    if !addons.is_empty() {
        web.insert(
            Value::from("depends_on"),
            Value::Sequence(
                addons
                    .iter()
                    .map(|addon| Value::from(addon.service_name.clone()))
                    .collect(),
            ),
        );
    }
    services.insert(Value::from(web_service), Value::Mapping(web));

    let mut volumes = Mapping::new();
    for addon in addons {
        let mut service = Mapping::new();
        service.insert(Value::from("image"), Value::from(addon.image.clone()));
        if !addon.env.is_empty() {
            let mut env = Mapping::new();
            for entry in &addon.env {
                env.insert(
                    Value::from(entry.key.clone()),
                    Value::from(format!("${{{}}}", entry.key)),
                );
            }
            service.insert(Value::from("environment"), Value::Mapping(env));
        }
        if !addon.volumes.is_empty() {
            service.insert(
                Value::from("volumes"),
                Value::Sequence(
                    addon
                        .volumes
                        .iter()
                        .map(|volume| Value::from(volume.clone()))
                        .collect(),
                ),
            );
            for volume in &addon.volumes {
                if let Some(name) = volume.split(':').next() {
                    volumes.insert(Value::from(name), Value::Null);
                }
            }
        }
        services.insert(
            Value::from(addon.service_name.clone()),
            Value::Mapping(service),
        );
    }

    let mut root = Mapping::new();
    root.insert(Value::from("services"), Value::Mapping(services));
    if !volumes.is_empty() {
        root.insert(Value::from("volumes"), Value::Mapping(volumes));
    }
    let compose = serde_yaml::to_string(&Value::Mapping(root)).unwrap_or_default();

    GeneratedCompose {
        compose_file: "compose.generated.hostlet.yml".to_string(),
        web_service: web_service.to_string(),
        port,
        health_path: health_path.to_string(),
        compose,
    }
}

/// The outcome of resolving requested add-ons: the `runtime_config` with
/// `generatedCompose` filled in, and the `(key, value)` env pairs to persist
/// (encrypted by the caller).
pub struct ResolvedAddons {
    pub runtime_config: serde_json::Value,
    pub env: Vec<(String, String)>,
}

/// Resolves `runtime_config.compose.addOns` (selected at app-create time) into a
/// generated multi-service Compose runtime plus the env to persist. Shared by
/// the self-hosted and Hostlet Cloud create handlers so the secret +
/// compose-generation logic lives in one place.
///
/// `secret_gen` mints a strong random value for each `generate`-flagged add-on
/// env key (the caller supplies its own CSPRNG; contracts stays dependency
/// light). The generated secrets land only in the returned `env`; the compose
/// references them via `${VAR}` interpolation, never plaintext in
/// `runtime_config`. Returns `Ok(None)` when no add-ons are requested.
pub fn resolve_managed_addons(
    runtime_config: &serde_json::Value,
    web_service: &str,
    port: u16,
    health_path: &str,
    mut secret_gen: impl FnMut() -> String,
) -> Result<Option<ResolvedAddons>, String> {
    let Some(requested) = runtime_config
        .pointer("/compose/addOns")
        .and_then(|value| value.as_array())
        .filter(|list| !list.is_empty())
    else {
        return Ok(None);
    };
    let catalog = add_on_catalog();
    let mut chosen: Vec<AddOn> = Vec::new();
    let mut env: Vec<(String, String)> = Vec::new();
    for item in requested {
        let key = item
            .get("key")
            .and_then(|value| value.as_str())
            .ok_or("each add-on requires a key")?;
        let addon = catalog
            .iter()
            .find(|candidate| candidate.key == key)
            .ok_or_else(|| format!("unknown add-on {key}"))?;
        if chosen
            .iter()
            .any(|existing| existing.service_name == addon.service_name)
        {
            return Err(format!("add-on {key} was requested more than once"));
        }
        for entry in &addon.env {
            let value = if entry.generate {
                secret_gen()
            } else {
                entry.default.clone().unwrap_or_default()
            };
            addon_env_upsert(&mut env, &entry.key, value);
        }
        for inject in &addon.inject {
            let rendered = render_addon_template(&inject.template, &env);
            addon_env_upsert(&mut env, &inject.key, rendered);
        }
        chosen.push(addon.clone());
    }
    let generated = generate_compose(web_service, port, health_path, &chosen);
    let mut runtime_config = runtime_config.clone();
    let object = runtime_config
        .as_object_mut()
        .ok_or("runtime config must be an object")?;
    object.insert(
        "generatedCompose".to_string(),
        serde_json::to_value(&generated).map_err(|_| "failed to encode generated compose")?,
    );
    Ok(Some(ResolvedAddons {
        runtime_config,
        env,
    }))
}

/// Inserts or replaces `key`, keeping the env list free of duplicates (a later
/// add-on reusing a var name wins, matching compose's last-write semantics).
fn addon_env_upsert(env: &mut Vec<(String, String)>, key: &str, value: String) {
    match env.iter_mut().find(|(existing, _)| existing == key) {
        Some(slot) => slot.1 = value,
        None => env.push((key.to_string(), value)),
    }
}

/// Substitutes `${VAR}` references in a connection-string template with the
/// already-resolved env values.
fn render_addon_template(template: &str, env: &[(String, String)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in env {
        rendered = rendered.replace(&format!("${{{key}}}"), value);
    }
    rendered
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
    fn catalog_has_postgres_and_redis() {
        let catalog = add_on_catalog();
        let keys: Vec<&str> = catalog.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains(&"postgres"));
        assert!(keys.contains(&"redis"));
        let postgres = catalog.iter().find(|a| a.key == "postgres").unwrap();
        // Postgres mints a password but never declares a host port.
        assert!(postgres
            .env
            .iter()
            .any(|e| e.key == "POSTGRES_PASSWORD" && e.generate));
        assert!(postgres.inject.iter().any(|i| i.key == "DATABASE_URL"));
    }

    #[test]
    fn generated_compose_is_within_the_safe_subset() {
        let catalog = add_on_catalog();
        let generated = generate_compose("web", 3000, "/", &catalog);
        // The generated stack must pass the very gate the agent enforces.
        assert!(
            compose_subset_warnings(&generated.compose, "web").is_empty(),
            "generated compose left the safe subset: {}",
            generated.compose
        );
        assert_eq!(generated.web_service, "web");
        assert_eq!(generated.compose_file, "compose.generated.hostlet.yml");
    }

    #[test]
    fn generated_compose_references_built_image_and_interpolated_secrets() {
        let postgres = add_on_catalog()
            .into_iter()
            .filter(|a| a.key == "postgres")
            .collect::<Vec<_>>();
        let generated = generate_compose("web", 8080, "/health", &postgres);
        // Web image is deferred to deploy time; secrets are interpolation refs,
        // never literal values baked into the stored compose.
        assert!(generated.compose.contains("${HOSTLET_WEB_IMAGE}"));
        assert!(generated.compose.contains("${POSTGRES_PASSWORD}"));
        assert!(generated.compose.contains("postgres:16-alpine"));
        assert!(generated.compose.contains("pgdata"));
        // No service publishes a host port in the generated stack.
        assert!(!generated.compose.contains("ports:"));
        // The parsed view tags postgres as a backing service.
        let services = parse_compose_services(&generated.compose, "web");
        assert_eq!(
            services.iter().find(|s| s.name == "postgres").unwrap().role,
            "backing"
        );
    }

    #[test]
    fn resolve_no_addons_returns_none() {
        assert!(
            resolve_managed_addons(&serde_json::json!({}), "web", 3000, "/", || "x".into())
                .unwrap()
                .is_none()
        );
        assert!(resolve_managed_addons(
            &serde_json::json!({"compose":{"addOns":[]}}),
            "web",
            3000,
            "/",
            || "x".into()
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn resolve_postgres_generates_secret_and_connection_string() {
        let resolved = resolve_managed_addons(
            &serde_json::json!({"compose":{"addOns":[{"key":"postgres"}]}}),
            "web",
            3000,
            "/",
            || "s3cret".into(),
        )
        .unwrap()
        .unwrap();
        let env: std::collections::HashMap<_, _> = resolved.env.iter().cloned().collect();
        assert_eq!(
            env.get("POSTGRES_PASSWORD").map(String::as_str),
            Some("s3cret")
        );
        assert_eq!(env.get("POSTGRES_DB").map(String::as_str), Some("app"));
        assert_eq!(
            env.get("DATABASE_URL").map(String::as_str),
            Some("postgres://postgres:s3cret@postgres:5432/app")
        );
        let compose = resolved
            .runtime_config
            .pointer("/generatedCompose/compose")
            .and_then(|value| value.as_str())
            .unwrap();
        assert!(compose_subset_warnings(compose, "web").is_empty());
        // The secret lives only in env; the stored compose keeps the ${VAR} ref.
        assert!(compose.contains("${POSTGRES_PASSWORD}"));
        assert!(!compose.contains("s3cret"));
    }

    #[test]
    fn resolve_rejects_unknown_and_duplicate_addons() {
        assert!(resolve_managed_addons(
            &serde_json::json!({"compose":{"addOns":[{"key":"mongo"}]}}),
            "web",
            3000,
            "/",
            || "x".into()
        )
        .is_err());
        assert!(resolve_managed_addons(
            &serde_json::json!({"compose":{"addOns":[{"key":"postgres"},{"key":"postgres"}]}}),
            "web",
            3000,
            "/",
            || "x".into()
        )
        .is_err());
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
