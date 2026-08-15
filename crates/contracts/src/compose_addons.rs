use serde::Serialize;

/// The compose-interpolated environment variable the agent sets to the freshly
/// built web image at deploy time.
pub const WEB_IMAGE_ENV: &str = "HOSTLET_WEB_IMAGE";

/// One environment variable a managed add-on's container needs. `generate`
/// secrets are minted per app at create time; the rest fall back to `default`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOnEnv {
    pub key: String,
    pub generate: bool,
    pub default: Option<String>,
}

/// A connection value injected into the web service, rendered from add-on env.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOnInject {
    pub key: String,
    pub template: String,
}

/// A managed backing-service add-on from the built-in catalog.
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
/// entry's image is vetted to run under Hostlet's per-service hardening.
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

/// The generated Compose runtime for a managed-add-ons app.
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

/// The outcome of resolving requested add-ons.
pub struct ResolvedAddons {
    pub runtime_config: serde_json::Value,
    pub env: Vec<(String, String)>,
}

/// Resolves selected add-ons into a generated multi-service Compose runtime.
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

fn addon_env_upsert(env: &mut Vec<(String, String)>, key: &str, value: String) {
    match env.iter_mut().find(|(existing, _)| existing == key) {
        Some(slot) => slot.1 = value,
        None => env.push((key.to_string(), value)),
    }
}

fn render_addon_template(template: &str, env: &[(String, String)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in env {
        rendered = rendered.replace(&format!("${{{key}}}"), value);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::super::{compose_subset_warnings, parse_compose_services};
    use super::*;

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
}
