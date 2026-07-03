use serde::Serialize;
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

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSuggestion {
    pub command: String,
    pub source_file: String,
    pub source_detail: String,
    pub confidence: &'static str,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RepoCommandFiles<'a> {
    pub procfile: Option<&'a str>,
    pub package_json: Option<&'a str>,
    pub railway_json: Option<&'a str>,
    pub render_yaml: Option<(&'a str, &'a str)>,
    pub dockerfile: Option<&'a str>,
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

pub fn detect_start_command(files: RepoCommandFiles<'_>) -> Option<CommandSuggestion> {
    files
        .procfile
        .and_then(procfile_start_command)
        .or_else(|| files.package_json.and_then(package_json_start_command))
        .or_else(|| files.railway_json.and_then(railway_start_command))
        .or_else(|| {
            files
                .render_yaml
                .and_then(|(source_file, contents)| render_start_command(source_file, contents))
        })
        .or_else(|| files.dockerfile.and_then(dockerfile_start_command))
}

fn command_suggestion(
    command: impl Into<String>,
    source_file: &str,
    source_detail: &str,
    confidence: &'static str,
) -> Option<CommandSuggestion> {
    let command = command.into();
    let command = crate::clean_command(Some(command)).ok().flatten()?;
    Some(CommandSuggestion {
        command,
        source_file: source_file.to_string(),
        source_detail: source_detail.to_string(),
        confidence,
    })
}

fn procfile_start_command(contents: &str) -> Option<CommandSuggestion> {
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((process, command)) = line.split_once(':') else {
            continue;
        };
        if process.trim() == "web" {
            return command_suggestion(command.trim(), "Procfile", "web", "high");
        }
    }
    None
}

fn package_json_start_command(contents: &str) -> Option<CommandSuggestion> {
    let package: Value = serde_json::from_str(contents).ok()?;
    let scripts = package.get("scripts").and_then(Value::as_object)?;
    let package_manager = infer_package_manager(contents, false, false, false);
    for script in ["start", "preview", "serve"] {
        let Some(command) = scripts.get(script).and_then(Value::as_str) else {
            continue;
        };
        if command.trim().is_empty() {
            continue;
        }
        crate::clean_command(Some(command.to_string()))
            .ok()
            .flatten()?;
        return command_suggestion(
            package_script_command(package_manager, script),
            "package.json",
            &format!("scripts.{script}"),
            if script == "start" { "high" } else { "medium" },
        );
    }
    None
}

fn package_script_command(package_manager: &str, script: &str) -> String {
    if package_manager == "bun" {
        format!("bun run {script}")
    } else {
        format!("{package_manager} run {script}")
    }
}

fn railway_start_command(contents: &str) -> Option<CommandSuggestion> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let (command, detail) = value
        .pointer("/deploy/startCommand")
        .and_then(Value::as_str)
        .map(|command| (command, "deploy.startCommand"))
        .or_else(|| {
            value
                .get("startCommand")
                .and_then(Value::as_str)
                .map(|command| (command, "startCommand"))
        })?;
    command_suggestion(command, "railway.json", detail, "medium")
}

fn render_start_command(source_file: &str, contents: &str) -> Option<CommandSuggestion> {
    let value: serde_yaml::Value = serde_yaml::from_str(contents).ok()?;
    let services = value
        .get("services")
        .and_then(|value| value.as_sequence())?;
    for (index, service) in services.iter().enumerate() {
        let Some(command) = service.get("startCommand").and_then(|value| value.as_str()) else {
            continue;
        };
        return command_suggestion(
            command,
            source_file,
            &format!("services[{index}].startCommand"),
            "medium",
        );
    }
    None
}

fn dockerfile_start_command(contents: &str) -> Option<CommandSuggestion> {
    let mut latest = None;
    for line in contents.lines().map(str::trim) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("CMD ") {
            latest = Some(("CMD", line[4..].trim()));
        } else if upper.starts_with("ENTRYPOINT ") {
            latest = Some(("ENTRYPOINT", line[10..].trim()));
        }
    }
    let (instruction, raw) = latest?;
    let command = docker_instruction_command(raw)?;
    command_suggestion(command, "Dockerfile", instruction, "low")
}

fn docker_instruction_command(raw: &str) -> Option<String> {
    if raw.starts_with('[') {
        let parts: Vec<String> = serde_json::from_str(raw).ok()?;
        let command = parts.join(" ");
        return (!command.trim().is_empty()).then_some(command);
    }
    (!raw.trim().is_empty()).then(|| raw.to_string())
}

pub fn with_command_suggestion(
    mut inspection: Value,
    suggestion: Option<CommandSuggestion>,
) -> Value {
    let Some(suggestion) = suggestion else {
        return inspection;
    };
    let Some(map) = inspection.as_object_mut() else {
        return inspection;
    };
    map.insert(
        "commandSuggestions".to_string(),
        serde_json::json!({ "start": suggestion }),
    );
    inspection
}

/// The backing services inferred from a repo (its dependency manifests, or a bare
/// compose file's images): the managed catalog add-ons Hostlet will provision,
/// plus notes for services it detected but has no managed offering for yet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetectedServices {
    /// Catalog add-on keys to provision, deduped and in catalog order.
    pub addons: Vec<String>,
    /// Skip notes for detected services with no managed catalog entry.
    pub warnings: Vec<String>,
}

impl DetectedServices {
    /// Unions another detection result in (e.g. dependency signals plus compose
    /// image signals), keeping add-ons deduped and in catalog order.
    pub fn merge(&mut self, other: &DetectedServices) {
        for addon in &other.addons {
            if !self.addons.contains(addon) {
                self.addons.push(addon.clone());
            }
        }
        for warning in &other.warnings {
            if !self.warnings.contains(warning) {
                self.warnings.push(warning.clone());
            }
        }
        self.sort_to_catalog_order();
    }

    fn sort_to_catalog_order(&mut self) {
        let catalog = crate::compose::add_on_catalog();
        self.addons.sort_by_key(|key| {
            catalog
                .iter()
                .position(|addon| &addon.key == key)
                .unwrap_or(usize::MAX)
        });
    }
}

/// A dependency identifier or image base-name (lowercased) that implies a
/// backing service. `exact` requires the whole identifier to equal the needle
/// (for short, ambiguous names like `pg`); otherwise the needle is matched as a
/// substring, so versioned module paths (`github.com/jackc/pgx/v5`), scoped
/// packages (`@prisma/client`), and tag-stripped images (`postgres`) still match.
struct ServiceSignal {
    needle: &'static str,
    service: &'static str,
    exact: bool,
}

/// Dependency/image → backing-service signals.
///
/// Heuristic note: the generic SQL ORMs (Prisma/Sequelize/TypeORM/Drizzle/Knex)
/// can target several engines, but Hostlet maps them to managed Postgres — the
/// catalog's SQL default and by far the most common choice. Detection only ever
/// *suggests*; the create preview shows exactly what was found so the user can
/// confirm (or change the repo) before deploying.
const SERVICE_SIGNALS: &[ServiceSignal] = &[
    ServiceSignal {
        needle: "pg",
        service: "postgres",
        exact: true,
    },
    ServiceSignal {
        needle: "pg-promise",
        service: "postgres",
        exact: true,
    },
    ServiceSignal {
        needle: "postgres",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "postgresql",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "psycopg",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "asyncpg",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "tokio-postgres",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "lib/pq",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "pgx",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "prisma",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "sequelize",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "typeorm",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "drizzle-orm",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "knex",
        service: "postgres",
        exact: false,
    },
    ServiceSignal {
        needle: "redis",
        service: "redis",
        exact: false,
    },
    ServiceSignal {
        needle: "bull",
        service: "redis",
        exact: true,
    },
    ServiceSignal {
        needle: "bullmq",
        service: "redis",
        exact: true,
    },
    // Detected, but Hostlet has no managed catalog add-on yet → skip + warn.
    ServiceSignal {
        needle: "mongoose",
        service: "mongodb",
        exact: false,
    },
    ServiceSignal {
        needle: "mongodb",
        service: "mongodb",
        exact: false,
    },
    ServiceSignal {
        needle: "mongo",
        service: "mongodb",
        exact: false,
    },
    ServiceSignal {
        needle: "mysql",
        service: "mysql",
        exact: false,
    },
    ServiceSignal {
        needle: "mariadb",
        service: "mysql",
        exact: false,
    },
];

/// Lowercased `dependencies` + `devDependencies` names from a `package.json`.
/// Returns an empty set for unparseable JSON — detection treats "no manifest" and
/// "no deps" the same (no services inferred).
pub fn package_json_dependencies(contents: &str) -> std::collections::HashSet<String> {
    let package: Value = serde_json::from_str(contents).unwrap_or_else(|_| serde_json::json!({}));
    let mut names = std::collections::HashSet::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(map) = package.get(key).and_then(Value::as_object) {
            names.extend(map.keys().map(|name| name.to_ascii_lowercase()));
        }
    }
    names
}

/// Lowercased dependency-ish tokens from a free-text manifest (requirements.txt,
/// go.mod, Cargo.toml, pyproject.toml, Gemfile). Splits on characters that never
/// occur inside a package name or module path, keeping `/ . - _ @` so versioned
/// module paths and scoped packages survive intact for substring matching.
pub fn manifest_dependency_tokens(contents: &str) -> std::collections::HashSet<String> {
    contents
        .to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '@')))
        .filter(|token| token.len() >= 2)
        .map(str::to_string)
        .collect()
}

/// Infers the managed backing services a repo needs from a set of dependency
/// identifiers (package.json deps, manifest tokens, or compose image names).
/// Emits only catalog add-on keys, in catalog order; services with no managed
/// catalog entry become skip warnings, so the create resolver never sees an
/// unknown add-on key.
pub fn infer_service_addons(identifiers: &std::collections::HashSet<String>) -> DetectedServices {
    let mut services: Vec<&'static str> = Vec::new();
    for signal in SERVICE_SIGNALS {
        let matched = identifiers.iter().any(|id| {
            if signal.exact {
                id == signal.needle
            } else {
                id.contains(signal.needle)
            }
        });
        if matched && !services.contains(&signal.service) {
            services.push(signal.service);
        }
    }
    let catalog = crate::compose::add_on_catalog();
    let mut detected = DetectedServices::default();
    for service in services {
        if catalog.iter().any(|addon| addon.key == service) {
            detected.addons.push(service.to_string());
        } else {
            detected.warnings.push(format!(
                "Detected a {service} dependency, but Hostlet has no managed {service} add-on yet, so it was skipped. Bring your own compose to run it yourself."
            ));
        }
    }
    detected.sort_to_catalog_order();
    detected
}

/// Infers managed add-ons from the service images declared in a bare
/// docker-compose file (one without a `hostlet.yml`). The repo's own images are
/// never run on a shared host — the file is read only as a signal of which
/// backing services the app needs; the web service is always rebuilt from the
/// repo via Railpack and its backing services come from the vetted catalog.
pub fn infer_addons_from_compose(compose_yaml: &str) -> DetectedServices {
    let services = crate::compose::parse_compose_services(compose_yaml, "");
    let identifiers: std::collections::HashSet<String> = services
        .iter()
        .filter_map(|service| service.image.as_deref())
        .map(|image| {
            image
                .split(':')
                .next()
                .unwrap_or(image)
                .to_ascii_lowercase()
        })
        .collect();
    infer_service_addons(&identifiers)
}

/// Overlays auto-detected managed services onto a single-runtime inspection
/// (Node/Dockerfile/Railpack): flips it to a Compose runtime, attaches the
/// catalog add-ons to `runtimeConfig.compose.addOns` (so the create handler
/// provisions them via `resolve_managed_addons`), and builds a service preview
/// (the repo-built web service plus each managed add-on) for the create UI. Skip
/// notes for unsupported services are surfaced even when no add-on is added.
pub fn with_detected_services(inspection: Value, detected: &DetectedServices) -> Value {
    let Value::Object(mut map) = inspection else {
        return inspection;
    };
    let mut notes = detected.warnings.clone();
    if !detected.addons.is_empty() {
        let catalog = crate::compose::add_on_catalog();
        let chosen: Vec<crate::compose::AddOn> = detected
            .addons
            .iter()
            .filter_map(|key| catalog.iter().find(|addon| &addon.key == key).cloned())
            .collect();
        let mut services = vec![crate::compose::ServiceSummary {
            name: "web".to_string(),
            role: "web".to_string(),
            image: None,
            build: true,
            ports: Vec::new(),
            volumes: Vec::new(),
        }];
        for addon in &chosen {
            services.push(crate::compose::ServiceSummary {
                name: addon.service_name.clone(),
                role: "backing".to_string(),
                image: Some(addon.image.clone()),
                build: false,
                ports: Vec::new(),
                volumes: addon.volumes.clone(),
            });
        }
        let names = chosen
            .iter()
            .map(|addon| addon.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let add_ons: Vec<Value> = detected
            .addons
            .iter()
            .map(|key| serde_json::json!({ "key": key }))
            .collect();
        map.insert("runtimeKind".to_string(), serde_json::json!("compose"));
        map.insert("webService".to_string(), serde_json::json!("web"));
        map.insert(
            "services".to_string(),
            serde_json::to_value(&services).unwrap_or_else(|_| serde_json::json!([])),
        );
        map.insert(
            "runtimeConfig".to_string(),
            serde_json::json!({ "compose": { "addOns": add_ons } }),
        );
        let one = chosen.len() == 1;
        notes.insert(
            0,
            format!(
                "Hostlet auto-detected {names} from your dependencies and will run {} as a managed service{}, injecting connection details (e.g. DATABASE_URL) into your app.",
                if one { "it" } else { "them" },
                if one { "" } else { "s" },
            ),
        );
    }
    if !notes.is_empty() {
        if let Some(Value::Array(warnings)) = map.get_mut("warnings") {
            for note in notes {
                warnings.push(serde_json::json!(note));
            }
        }
    }
    Value::Object(map)
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

#[cfg(test)]
mod tests;
