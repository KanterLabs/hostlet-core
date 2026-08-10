use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum RouteShape {
    Single,
    Split,
    Invalid,
}

/// Generated topology is immutable for a deployment, so its receipt and route
/// declaration must agree before the agent may rewrite that route. Missing,
/// unknown, or duplicate service roles fail closed through `RouteShape::Invalid`.
pub(super) fn route_shape(metadata: &serde_json::Value, row: &sqlx::postgres::PgRow) -> RouteShape {
    let backend_count = row.get::<i64, _>("backend_service_count");
    let valid_backend_name = row
        .get::<Option<String>, _>("backend_service_name")
        .is_some_and(|name| !name.is_empty() && name.len() <= 64);
    if metadata.get("runtime").and_then(serde_json::Value::as_str) != Some("generated_topology") {
        return match (backend_count, valid_backend_name) {
            (0, _) => RouteShape::Single,
            (1, true) => RouteShape::Split,
            _ => RouteShape::Invalid,
        };
    }

    let Some(services) = metadata
        .pointer("/inferenceReceipt/services")
        .and_then(serde_json::Value::as_array)
        .filter(|services| !services.is_empty())
    else {
        return RouteShape::Invalid;
    };
    if !services.iter().all(valid_inferred_service) {
        return RouteShape::Invalid;
    }
    match metadata
        .pointer("/routing/kind")
        .and_then(serde_json::Value::as_str)
    {
        Some("single") if backend_count == 0 => RouteShape::Single,
        Some("split") if backend_count == 1 && valid_backend_name => RouteShape::Split,
        _ => RouteShape::Invalid,
    }
}

fn valid_inferred_service(service: &serde_json::Value) -> bool {
    let valid_name = service
        .get("name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|name| !name.is_empty() && name.len() <= 64);
    let valid_role = service
        .get("role")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|role| matches!(role, "frontend" | "backend" | "web"));
    valid_name && valid_role
}

/// Build split-route facts from the backend selected by the inference receipt.
/// Both generated services use `role='web'` in deployment-service storage, so
/// the validated receipt role is the only reliable backend selector.
pub(super) fn split_route_payload(row: &sqlx::postgres::PgRow) -> Option<serde_json::Value> {
    let backend_container = row.get::<Option<String>, _>("backend_container_name")?;
    let backend_target_port = row.get::<Option<i32>, _>("backend_target_port")?;
    let backend_published_port = row.get::<Option<i32>, _>("backend_published_port")?;
    let backend_service_name = row.get::<Option<String>, _>("backend_service_name")?;
    if !crate::agent::valid_container_name(&backend_container)
        || !(1..=65_535).contains(&backend_target_port)
        || !(1..=65_535).contains(&backend_published_port)
        || backend_service_name.is_empty()
        || backend_service_name.len() > 64
    {
        return None;
    }

    let metadata = row.get::<serde_json::Value, _>("runtime_metadata");
    let prefixes = metadata
        .pointer("/inferenceReceipt/routing/backendPathPrefixes")
        .or_else(|| metadata.pointer("/routing/backendPathPrefixes"))
        .and_then(valid_backend_path_prefixes)?;
    Some(serde_json::json!({
        "backend": {
            "serviceName": backend_service_name,
            "containerName": backend_container,
            "targetPort": backend_target_port,
            "publishedPort": backend_published_port,
        },
        "backendPathPrefixes": prefixes,
    }))
}

fn valid_backend_path_prefixes(value: &serde_json::Value) -> Option<Vec<String>> {
    let values = value.as_array()?;
    if values.len() > 16 {
        return None;
    }
    let mut prefixes = Vec::with_capacity(values.len());
    for value in values {
        let prefix = value.as_str()?.to_string();
        if !valid_backend_path_prefix(&prefix) || prefixes.iter().any(|item| item == &prefix) {
            return None;
        }
        prefixes.push(prefix);
    }
    Some(prefixes)
}

fn valid_backend_path_prefix(value: &str) -> bool {
    value.starts_with('/')
        && value != "/"
        && value.len() <= 128
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains('*')
        && !value.contains('?')
        && !value.contains('#')
        && !value.chars().any(|ch| ch.is_control() || ch == '\\')
}
