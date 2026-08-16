use serde::Serialize;

use super::{map_get, map_has, string_seq};

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
