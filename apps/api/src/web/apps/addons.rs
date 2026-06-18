//! Resolves managed add-ons selected at app-create time into a generated
//! multi-service Compose runtime plus the env vars to persist.
//!
//! Add-on secrets are generated here and stored in the app's **encrypted** env;
//! the generated Compose references them via `${VAR}` interpolation, so no
//! secret is ever written into `runtime_config`. Connection strings (e.g.
//! `DATABASE_URL`) are rendered to concrete values for the web service.

use hostlet_contracts::compose::{add_on_catalog, generate_compose, AddOn};

/// The outcome of resolving requested add-ons: the `runtime_config` with
/// `generatedCompose` filled in, and the `(key, value)` env pairs to persist.
pub(super) struct ResolvedAddons {
    pub(super) runtime_config: serde_json::Value,
    pub(super) env: Vec<(String, String)>,
}

/// Resolves `runtime_config.compose.addOns` into a generated multi-service
/// runtime. Returns `Ok(None)` when no add-ons are requested. Errors are
/// user-facing 400 messages.
pub(super) fn resolve_managed_addons(
    runtime_config: &serde_json::Value,
    web_service: &str,
    port: u16,
    health_path: &str,
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
                crate::crypto::random_token(32)
            } else {
                entry.default.clone().unwrap_or_default()
            };
            upsert(&mut env, &entry.key, value);
        }
        for inject in &addon.inject {
            let rendered = render_template(&inject.template, &env);
            upsert(&mut env, &inject.key, rendered);
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
/// add-on that reuses a var name wins, matching compose's last-write semantics).
fn upsert(env: &mut Vec<(String, String)>, key: &str, value: String) {
    match env.iter_mut().find(|(existing, _)| existing == key) {
        Some(slot) => slot.1 = value,
        None => env.push((key.to_string(), value)),
    }
}

/// Substitutes `${VAR}` references in a connection-string template with the
/// already-resolved env values.
fn render_template(template: &str, env: &[(String, String)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in env {
        rendered = rendered.replace(&format!("${{{key}}}"), value);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn no_addons_returns_none() {
        assert!(
            resolve_managed_addons(&serde_json::json!({}), "web", 3000, "/")
                .unwrap()
                .is_none()
        );
        assert!(resolve_managed_addons(
            &serde_json::json!({"compose":{"addOns":[]}}),
            "web",
            3000,
            "/"
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn postgres_addon_resolves_secret_connection_string_and_compose() {
        let runtime_config = serde_json::json!({"compose":{"addOns":[{"key":"postgres"}]}});
        let resolved = resolve_managed_addons(&runtime_config, "web", 3000, "/")
            .unwrap()
            .unwrap();
        let env: HashMap<_, _> = resolved.env.iter().cloned().collect();

        let password = env.get("POSTGRES_PASSWORD").expect("password generated");
        assert!(!password.is_empty());
        assert_eq!(env.get("POSTGRES_DB").map(String::as_str), Some("app"));
        let url = env.get("DATABASE_URL").expect("connection string");
        assert!(url.starts_with("postgres://postgres:"));
        assert!(url.ends_with("@postgres:5432/app"));
        // The rendered URL embeds the concrete password (no leftover ${...}).
        assert!(url.contains(password));
        assert!(!url.contains("${"));

        // The generated compose is stored and within the safe subset; the
        // secret lives only in env, never in the compose YAML.
        let compose = resolved
            .runtime_config
            .pointer("/generatedCompose/compose")
            .and_then(|value| value.as_str())
            .expect("generated compose stored");
        assert!(hostlet_contracts::compose::compose_subset_warnings(compose, "web").is_empty());
        assert!(compose.contains("${POSTGRES_PASSWORD}"));
        assert!(!compose.contains(password.as_str()));
    }

    #[test]
    fn unknown_addon_is_rejected() {
        let runtime_config = serde_json::json!({"compose":{"addOns":[{"key":"mongo"}]}});
        assert!(resolve_managed_addons(&runtime_config, "web", 3000, "/").is_err());
    }

    #[test]
    fn duplicate_addon_is_rejected() {
        let runtime_config =
            serde_json::json!({"compose":{"addOns":[{"key":"postgres"},{"key":"postgres"}]}});
        assert!(resolve_managed_addons(&runtime_config, "web", 3000, "/").is_err());
    }
}
