use serde_json::Value;

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
    has_dockerfile: bool,
) -> Value {
    let mut result = object_map(inspection_base(InspectionBaseInput {
        repo,
        branch,
        default_branch,
        deployable: true,
        container_port: serde_json::json!(3000),
        packaging_options: if has_dockerfile {
            serde_json::json!(["auto", "generated", "dockerfile"])
        } else {
            serde_json::json!(["auto", "generated"])
        },
        recommended_packaging_strategy: "generated",
        env: serde_json::json!([]),
        warnings: if has_dockerfile {
            serde_json::json!([
                "Node app and Dockerfile detected. Hostlet recommends Railpack generated runtime; select repository Dockerfile only when the app depends on custom image setup.",
                "Set custom build/start commands if the Railpack preview is incomplete."
            ])
        } else {
            serde_json::json!([
                "Node app detected. Hostlet will build it with Railpack. Set custom build/start commands if the preview is incomplete."
            ])
        },
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

/// Builds the inspection payload for a bring-your-own multi-service Compose app
/// (a repo with a `hostlet.yml` declaring `runtime: compose`). Surfaces the
/// parsed service list so the UI can render a card per service, marks the app
/// undeployable when the compose breaches the safe subset, and folds the subset
/// warnings into the standard `warnings` array the create UI already renders.
pub fn compose_inspection(
    repo: &str,
    branch: &str,
    default_branch: &str,
    hostlet_config_path: &str,
    compose: &crate::compose::HostletComposeSection,
    services: &[crate::compose::ServiceSummary],
    subset_warnings: &[String],
) -> Value {
    let web_service = compose.web_service.as_str();
    let container_port = compose.port.map(i32::from).unwrap_or(3000);
    let health_path = compose.health_path.as_deref().unwrap_or("/");
    let mut warnings = vec![format!(
        "Multi-service Compose app detected ({} services). Hostlet runs the '{web_service}' service as the web entrypoint; the other services are reachable only on the app's internal network.",
        services.len()
    )];
    warnings.extend(subset_warnings.iter().cloned());
    let mut result = object_map(inspection_base(InspectionBaseInput {
        repo,
        branch,
        default_branch,
        deployable: subset_warnings.is_empty(),
        container_port: serde_json::json!(container_port),
        packaging_options: serde_json::json!(["auto"]),
        recommended_packaging_strategy: "auto",
        env: serde_json::json!([]),
        warnings: serde_json::json!(warnings),
        summary: format!(
            "Compose app detected with {} services. Web service: {web_service}.",
            services.len()
        ),
    }));
    result.insert("runtimeKind".into(), serde_json::json!("compose"));
    result.insert("webService".into(), serde_json::json!(web_service));
    result.insert(
        "hostletConfigPath".into(),
        serde_json::json!(hostlet_config_path),
    );
    result.insert("healthPath".into(), serde_json::json!(health_path));
    result.insert(
        "services".into(),
        serde_json::to_value(services).unwrap_or_else(|_| serde_json::json!([])),
    );
    Value::Object(result)
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
