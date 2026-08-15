//! Shared zero-config repository topology inference.
//!
//! The API builds a bounded inventory from GitHub for preview and the agent
//! builds the same inventory from the immutable checkout before building.  The
//! planner is deliberately pure: callers provide paths and small text files;
//! it never reads the filesystem, executes commands, or trusts client-provided
//! commands/images.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[path = "topology_candidates.rs"]
mod topology_candidates;
use topology_candidates::{go_candidates, python_candidates, rust_candidates, static_candidates};

pub const GENERATED_TOPOLOGY_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_BACKEND_PATH_PREFIXES: &[&str] = &["/api", "/graphql", "/socket.io", "/trpc"];
pub const GENERATED_TOPOLOGY_ADDONS_WARNING: &str =
    "Managed Postgres/Redis add-ons cannot be combined with generated topology yet. Add a hostlet.yml Compose manifest to run the inferred services and backing services together.";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryFile {
    pub path: String,
    #[serde(default)]
    pub contents: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInventory {
    #[serde(default)]
    pub files: Vec<RepositoryFile>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyReadiness {
    Ready,
    NeedsSelection,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceConfidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRole {
    Frontend,
    Backend,
    Web,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthProbeKind {
    Http,
    Tcp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthProbe {
    pub kind: HealthProbeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceCandidate {
    /// Stable, user-selectable key derived from provider + manifest path/name.
    pub selector: String,
    pub name: String,
    pub role: ServiceRole,
    pub root_directory: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_directory: Option<String>,
    pub container_port: u16,
    pub health_probe: HealthProbe,
    #[serde(default)]
    pub public_env: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

pub type InferredService = ServiceCandidate;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferredRouting {
    pub websockets_to_backend: bool,
    #[serde(default)]
    pub backend_path_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyPlan {
    pub schema_version: u32,
    pub readiness: TopologyReadiness,
    pub confidence: InferenceConfidence,
    #[serde(default)]
    pub services: Vec<InferredService>,
    #[serde(default)]
    pub candidates: Vec<ServiceCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<InferredRouting>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedTopologyConfig {
    pub schema_version: u32,
    /// `auto` follows a future high-confidence plan; `selected` pins selectors.
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend_selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_selector: Option<String>,
    #[serde(default = "default_backend_prefixes")]
    pub backend_path_prefixes: Vec<String>,
}

fn default_backend_prefixes() -> Vec<String> {
    DEFAULT_BACKEND_PATH_PREFIXES
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}

impl Default for GeneratedTopologyConfig {
    fn default() -> Self {
        Self {
            schema_version: GENERATED_TOPOLOGY_SCHEMA_VERSION,
            mode: "auto".to_string(),
            frontend_selector: None,
            backend_selector: None,
            backend_path_prefixes: default_backend_prefixes(),
        }
    }
}

pub fn validate_generated_topology_config(
    config: &GeneratedTopologyConfig,
) -> Result<(), &'static str> {
    if config.schema_version != GENERATED_TOPOLOGY_SCHEMA_VERSION {
        return Err("unsupported generated topology schema version");
    }
    if !matches!(config.mode.as_str(), "auto" | "selected") {
        return Err("generated topology mode must be auto or selected");
    }
    if config.mode == "selected"
        && config.frontend_selector.is_none()
        && config.backend_selector.is_none()
    {
        return Err("selected generated topology requires a service selector");
    }
    for selector in [
        config.frontend_selector.as_deref(),
        config.backend_selector.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if selector.is_empty()
            || selector.len() > 512
            || selector.chars().any(char::is_control)
            || selector.contains("..")
        {
            return Err("generated topology selector is invalid");
        }
    }
    if config.backend_path_prefixes.len() > 16 {
        return Err("generated topology has too many backend path prefixes");
    }
    let mut seen = BTreeSet::new();
    for prefix in &config.backend_path_prefixes {
        if !valid_route_prefix(prefix) || !seen.insert(prefix) {
            return Err("generated topology backend path prefix is invalid");
        }
    }
    Ok(())
}

/// Returns whether a runtime config asks the generated-topology pipeline to
/// deploy alongside managed Compose add-ons. Those pipelines have different
/// service/network lifecycles, so accepting both would silently drop the
/// add-ons when the agent selects generated topology first.
pub fn runtime_config_has_generated_topology_addons(runtime_config: &Value) -> bool {
    runtime_config.get("generatedTopology").is_some()
        && runtime_config_has_managed_addons(runtime_config)
}

fn runtime_config_has_managed_addons(runtime_config: &Value) -> bool {
    runtime_config
        .pointer("/compose/addOns")
        .and_then(Value::as_array)
        .is_some_and(|add_ons| !add_ons.is_empty())
}

/// Rejects runtime combinations that cannot be represented by one agent
/// deployment pipeline. Keep this shared by API input validation and the agent
/// because queued jobs can outlive the inspection response or be replayed.
pub fn validate_runtime_config_compatibility(runtime_config: &Value) -> Result<(), &'static str> {
    if runtime_config_has_generated_topology_addons(runtime_config) {
        return Err("managed add-ons require an explicit Compose runtime; generated topology cannot deploy them together");
    }
    Ok(())
}

fn valid_route_prefix(value: &str) -> bool {
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

/// Adds the typed inference plan and, when ready, the safe automatic topology
/// selection to an existing repository-inspection response.
pub fn attach_topology_plan(mut inspection: Value, plan: &TopologyPlan) -> Value {
    let Some(map) = inspection.as_object_mut() else {
        return inspection;
    };
    let managed_addons = map
        .get("runtimeConfig")
        .is_some_and(runtime_config_has_managed_addons);
    let generated_topology_addons = managed_addons && plan.readiness == TopologyReadiness::Ready;
    map.insert(
        "inferencePlan".to_string(),
        serde_json::to_value(plan).unwrap_or_else(|_| serde_json::json!({})),
    );
    map.insert(
        "deployable".to_string(),
        serde_json::json!(plan.readiness == TopologyReadiness::Ready && !generated_topology_addons),
    );
    map.insert("summary".to_string(), serde_json::json!(plan.summary));
    if managed_addons && plan.readiness != TopologyReadiness::Unsupported {
        let warnings = map
            .entry("warnings")
            .or_insert_with(|| serde_json::json!([]));
        if let Some(warnings) = warnings.as_array_mut() {
            let warning = serde_json::json!(GENERATED_TOPOLOGY_ADDONS_WARNING);
            if !warnings.iter().any(|existing| existing == &warning) {
                warnings.push(warning);
            }
        }
    }
    if plan.readiness == TopologyReadiness::Ready {
        let config = GeneratedTopologyConfig::default();
        let runtime_config = map
            .entry("runtimeConfig")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(runtime_config) = runtime_config.as_object_mut() {
            runtime_config.insert(
                "generatedTopology".to_string(),
                serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({})),
            );
        }
        if plan.services.len() > 1 {
            map.insert("runtimeKind".to_string(), serde_json::json!("compose"));
        }
    }
    inspection
}

pub fn plan_repository_topology(inventory: &RepositoryInventory) -> TopologyPlan {
    let files = normalized_files(inventory);
    let manager = root_node_manager(&files);
    let mut candidates = Vec::new();

    for (path, contents) in files
        .iter()
        .filter(|(path, _)| path.ends_with("package.json"))
    {
        if let Some(candidate) = node_candidate(path, contents, manager) {
            candidates.push(candidate);
        }
    }
    candidates.extend(static_candidates(&files));
    candidates.extend(python_candidates(&files));
    candidates.extend(go_candidates(&files));
    candidates.extend(rust_candidates(&files));
    attach_public_env_evidence(&mut candidates, &files);
    candidates.sort_by(|a, b| a.selector.cmp(&b.selector));
    candidates.dedup_by(|a, b| a.selector == b.selector);

    let frontends: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.role == ServiceRole::Frontend)
        .cloned()
        .collect();
    let backends: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.role == ServiceRole::Backend)
        .cloned()
        .collect();
    let web: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.role == ServiceRole::Web)
        .cloned()
        .collect();

    let (readiness, confidence, services, summary, warnings) =
        match (frontends.len(), backends.len(), web.len()) {
            (1, 1, 0) => (
                TopologyReadiness::Ready,
                InferenceConfidence::High,
                vec![frontends[0].clone(), backends[0].clone()],
                format!(
                    "Hostlet found frontend {} and backend {}.",
                    frontends[0].root_directory, backends[0].root_directory
                ),
                Vec::new(),
            ),
            (1, 0, 0) => (
                TopologyReadiness::Ready,
                InferenceConfidence::High,
                vec![frontends[0].clone()],
                format!(
                    "Hostlet found a frontend in {}.",
                    frontends[0].root_directory
                ),
                Vec::new(),
            ),
            (0, 1, 0) => (
                TopologyReadiness::Ready,
                InferenceConfidence::High,
                vec![backends[0].clone()],
                format!("Hostlet found a backend in {}.", backends[0].root_directory),
                Vec::new(),
            ),
            (0, 0, 1) => (
                TopologyReadiness::Ready,
                InferenceConfidence::High,
                vec![web[0].clone()],
                format!(
                    "Hostlet found a runnable service in {}.",
                    web[0].root_directory
                ),
                Vec::new(),
            ),
            (0, 0, 0) => (
                TopologyReadiness::Unsupported,
                InferenceConfidence::Low,
                Vec::new(),
                "Hostlet could not find a recognizable runnable service.".to_string(),
                vec![
                    "Add an explicit start command or supported application manifest.".to_string(),
                ],
            ),
            _ => (
                TopologyReadiness::NeedsSelection,
                InferenceConfidence::Medium,
                Vec::new(),
                "Hostlet found multiple runnable service candidates.".to_string(),
                vec!["Choose at most one frontend and one backend before deploying.".to_string()],
            ),
        };

    let has_backend = services
        .iter()
        .any(|service| service.role == ServiceRole::Backend);
    TopologyPlan {
        schema_version: GENERATED_TOPOLOGY_SCHEMA_VERSION,
        readiness,
        confidence,
        services,
        candidates,
        routing: has_backend.then(|| InferredRouting {
            websockets_to_backend: true,
            backend_path_prefixes: default_backend_prefixes(),
        }),
        warnings,
        summary,
    }
}

fn normalized_files(inventory: &RepositoryInventory) -> BTreeMap<String, String> {
    inventory
        .files
        .iter()
        .filter_map(|file| {
            let path = file.path.trim_start_matches("./").replace('\\', "/");
            if path.is_empty() || is_noise_path(&path) {
                return None;
            }
            Some((path, file.contents.clone().unwrap_or_default()))
        })
        .collect()
}

fn is_noise_path(path: &str) -> bool {
    path.split('/').any(|part| {
        part.starts_with('.')
            || matches!(
                part,
                "node_modules"
                    | "dist"
                    | "build"
                    | "out"
                    | "target"
                    | "vendor"
                    | "coverage"
                    | "docs"
                    | "test"
                    | "tests"
                    | "fixtures"
                    | "examples"
                    | "example"
            )
    })
}

fn root_node_manager(files: &BTreeMap<String, String>) -> &'static str {
    let package = files.get("package.json").and_then(|contents| {
        serde_json::from_str::<Value>(contents)
            .ok()
            .and_then(|value| value.get("packageManager")?.as_str().map(str::to_string))
    });
    if let Some(package) = package {
        if package.starts_with("pnpm@") {
            return "pnpm";
        }
        if package.starts_with("yarn@") {
            return "yarn";
        }
        if package.starts_with("bun@") {
            return "bun";
        }
        if package.starts_with("npm@") {
            return "npm";
        }
    }
    if files.contains_key("pnpm-lock.yaml") || files.contains_key("pnpm-workspace.yaml") {
        "pnpm"
    } else if files.contains_key("yarn.lock") {
        "yarn"
    } else if files.contains_key("bun.lock") || files.contains_key("bun.lockb") {
        "bun"
    } else {
        "npm"
    }
}

fn node_candidate(
    manifest_path: &str,
    contents: &str,
    manager: &'static str,
) -> Option<ServiceCandidate> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let directory = parent_directory(manifest_path);
    let scripts = value.get("scripts").and_then(Value::as_object);
    let has_build = scripts.and_then(|scripts| scripts.get("build")).is_some();
    let start_script = ["start", "serve", "preview"]
        .into_iter()
        .find(|name| scripts.and_then(|scripts| scripts.get(*name)).is_some());
    let deps = ["dependencies", "devDependencies"]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_object))
        .flat_map(|deps| deps.keys().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let frontend_framework = [
        "vite",
        "react-scripts",
        "astro",
        "nuxt",
        "@sveltejs/kit",
        "next",
        "@remix-run/react",
    ]
    .into_iter()
    .find(|dependency| deps.contains(dependency));
    let backend_framework = [
        "ws",
        "express",
        "fastify",
        "koa",
        "hono",
        "@nestjs/core",
        "socket.io",
    ]
    .into_iter()
    .find(|dependency| deps.contains(dependency));
    if !has_build && start_script.is_none() {
        return None;
    }
    // A build-only package is a library/coordinator, not a runnable service.
    // This applies inside workspaces too: packages such as shared simulation,
    // protocol, and content libraries commonly expose `build` without a start
    // command. Static frontends remain candidates through their recognized
    // framework dependency.
    if start_script.is_none() && frontend_framework.is_none() {
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| directory.rsplit('/').next().unwrap_or("node-app"));
    let role = if backend_framework.is_some() {
        ServiceRole::Backend
    } else if frontend_framework.is_some() && start_script.is_none() {
        ServiceRole::Frontend
    } else {
        ServiceRole::Web
    };
    let workspace = directory != ".";
    let build_command = has_build.then(|| workspace_command(manager, name, "build", workspace));
    let start_command =
        start_script.map(|script| workspace_command(manager, name, script, workspace));
    let output_directory = (role == ServiceRole::Frontend).then(|| {
        let output = if deps.contains("next") {
            ".next"
        } else {
            "dist"
        };
        join_directory(&directory, output)
    });
    let public_env = node_public_env_candidates(contents, &directory);
    let mut evidence = Vec::new();
    if let Some(framework) = frontend_framework {
        evidence.push(format!("dependency {framework}"));
    }
    if let Some(framework) = backend_framework {
        evidence.push(format!("dependency {framework}"));
    }
    if let Some(script) = start_script {
        evidence.push(format!("scripts.{script}"));
    }
    if has_build {
        evidence.push("scripts.build".to_string());
    }
    Some(ServiceCandidate {
        selector: format!("node:{manifest_path}:{name}"),
        name: name.to_string(),
        role,
        root_directory: directory,
        provider: "node".to_string(),
        package_manager: Some(manager.to_string()),
        build_command,
        start_command,
        output_directory,
        container_port: if role == ServiceRole::Frontend {
            80
        } else {
            3000
        },
        health_probe: if role == ServiceRole::Frontend {
            HealthProbe {
                kind: HealthProbeKind::Http,
                path: Some("/health".to_string()),
            }
        } else if backend_framework == Some("ws") {
            HealthProbe {
                kind: HealthProbeKind::Tcp,
                path: None,
            }
        } else {
            HealthProbe {
                kind: HealthProbeKind::Http,
                path: Some("/".to_string()),
            }
        },
        public_env,
        evidence,
    })
}

fn workspace_command(manager: &str, name: &str, script: &str, workspace: bool) -> String {
    if !workspace {
        return match manager {
            "bun" => format!("bun run {script}"),
            _ => format!("{manager} run {script}"),
        };
    }
    match manager {
        "pnpm" if script == "build" => format!("pnpm --filter {name}... run build"),
        "pnpm" => format!("pnpm --filter {name} run {script}"),
        "npm" => format!("npm run {script} --workspace {name}"),
        "yarn" => format!("yarn workspace {name} run {script}"),
        "bun" => format!("bun run --filter {name} {script}"),
        _ => format!("{manager} run {script}"),
    }
}

fn node_public_env_candidates(_manifest: &str, _directory: &str) -> Vec<String> {
    Vec::new()
}

fn attach_public_env_evidence(
    candidates: &mut [ServiceCandidate],
    files: &BTreeMap<String, String>,
) {
    const PUBLIC_ENDPOINT_KEYS: &[&str] = &[
        "VITE_WS_URL",
        "VITE_API_URL",
        "VITE_BACKEND_URL",
        "NEXT_PUBLIC_WS_URL",
        "NEXT_PUBLIC_API_URL",
        "NEXT_PUBLIC_BACKEND_URL",
        "REACT_APP_WS_URL",
        "REACT_APP_API_URL",
        "PUBLIC_WS_URL",
        "PUBLIC_API_URL",
    ];
    for candidate in candidates
        .iter_mut()
        .filter(|candidate| candidate.role == ServiceRole::Frontend)
    {
        let prefix =
            (candidate.root_directory != ".").then(|| format!("{}/", candidate.root_directory));
        for (path, contents) in files {
            if !source_file(path)
                || prefix
                    .as_deref()
                    .is_some_and(|prefix| !path.starts_with(prefix))
            {
                continue;
            }
            for key in PUBLIC_ENDPOINT_KEYS {
                if contents.contains(key) && !candidate.public_env.iter().any(|item| item == key) {
                    candidate.public_env.push((*key).to_string());
                    candidate.evidence.push(format!("source references {key}"));
                }
            }
        }
        candidate.public_env.sort();
    }
}

fn source_file(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("js" | "jsx" | "ts" | "tsx" | "vue" | "svelte")
    )
}

fn toml_string(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate.trim() == key)
            .then(|| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    })
}

fn parent_directory(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(directory, _)| directory.to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn join_directory(directory: &str, path: &str) -> String {
    if directory == "." {
        path.to_string()
    } else {
        format!("{directory}/{path}")
    }
}

fn in_directory(directory: &str, command: &str) -> String {
    if directory == "." {
        command.to_string()
    } else {
        format!("cd {directory} && {command}")
    }
}

#[cfg(test)]
#[path = "topology_patchwork_tests.rs"]
mod patchwork_tests;

#[cfg(test)]
#[path = "topology/tests.rs"]
mod tests;
