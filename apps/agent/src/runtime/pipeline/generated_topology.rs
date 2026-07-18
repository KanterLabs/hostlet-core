use super::*;
use hostlet_contracts::{
    GeneratedTopologyConfig, HealthProbeKind, RepositoryFile, RepositoryInventory, ServiceRole,
    TopologyReadiness,
};
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const INVENTORY_MAX_FILES: usize = 10_000;
const INVENTORY_MAX_RELEVANT_FILES: usize = 1_024;
const INVENTORY_MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;

struct RunningService {
    service: hostlet_contracts::InferredService,
    image: String,
    container: String,
    published_port: u16,
    runtime_metadata: Value,
}

pub(crate) async fn rollback_generated_topology(
    cfg: &Config,
    payload: &Value,
) -> anyhow::Result<()> {
    let deployment_id =
        Uuid::parse_str(payload["deployment_id"].as_str().context("deployment_id")?)?;
    let domain = payload["domain"].as_str().context("domain")?;
    validate_domain(domain)?;
    let route_key = payload
        .get("route_key")
        .and_then(Value::as_str)
        .map(app_slug)
        .unwrap_or_else(|| app_slug(payload["app_id"].as_str().unwrap_or("app")));
    let receipt = payload
        .pointer("/target_runtime_metadata/inferenceReceipt")
        .context("rollback target is missing its inference receipt")?;
    let inferred = receipt
        .get("services")
        .and_then(Value::as_array)
        .context("rollback target is missing inferred services")?;
    let reports = payload
        .get("target_services")
        .and_then(Value::as_array)
        .context("rollback target is missing service records")?;
    if inferred.is_empty() || inferred.len() != reports.len() {
        bail!("rollback target has incomplete topology service records");
    }
    let mut frontend = None;
    let mut backend = None;
    for service in inferred {
        let name = service
            .get("name")
            .and_then(Value::as_str)
            .context("inferred service name")?;
        let role = service
            .get("role")
            .and_then(Value::as_str)
            .context("inferred service role")?;
        let report = reports
            .iter()
            .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
            .with_context(|| format!("rollback service record for {name} is missing"))?;
        let container = report
            .get("containerName")
            .and_then(Value::as_str)
            .context("rollback container")?;
        let port = report
            .get("publishedPort")
            .and_then(Value::as_u64)
            .filter(|port| (1..=65_535).contains(port))
            .context("rollback published port")? as u16;
        run_quiet("docker", &["start", container]).await?;
        let probe_kind = service
            .pointer("/healthProbe/kind")
            .and_then(Value::as_str)
            .unwrap_or("http");
        if probe_kind == "tcp" {
            wait_tcp_health(cfg, deployment_id, container, port).await?;
        } else {
            let path = service
                .pointer("/healthProbe/path")
                .and_then(Value::as_str)
                .unwrap_or("/");
            wait_health(cfg, deployment_id, container, port, path).await?;
        }
        if role == "frontend" {
            frontend = Some((container, port));
        }
        if role == "backend" {
            backend = Some((container, port));
        }
        if frontend.is_none() && backend.is_none() {
            frontend = Some((container, port));
        }
    }
    let primary = frontend
        .or(backend)
        .context("rollback topology has no routable service")?;
    let metadata = payload
        .get("target_runtime_metadata")
        .cloned()
        .unwrap_or_default();
    let service_reports = reports
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<hostlet_contracts::DeploymentServiceReport>, _>>()
        .context("rollback target has invalid service records")?;
    let route_generation = prepare_candidate_activation(
        cfg,
        payload,
        deployment_id,
        payload.get("target_image").and_then(Value::as_str),
        primary.0,
        primary.1,
        None,
        metadata,
        service_reports,
    )
    .await?;
    let prefixes = receipt
        .pointer("/routing/backendPathPrefixes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some((_, backend_port)) = backend {
        if cfg.local_mode {
            if let Some(router) = &cfg.local_router {
                apply_local_caddy_split_route_versioned(
                    cfg,
                    deployment_id,
                    router,
                    &route_key,
                    domain,
                    primary.1,
                    backend_port,
                    &prefixes,
                    route_generation,
                )
                .await?;
            }
        } else {
            apply_caddy_split_route_versioned(
                cfg,
                deployment_id,
                &route_key,
                domain,
                primary.1,
                backend_port,
                &prefixes,
                route_generation,
            )
            .await?;
        }
    } else if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route_versioned(
                cfg,
                deployment_id,
                router,
                &route_key,
                domain,
                primary.1,
                route_generation,
            )
            .await?;
        }
    } else {
        apply_caddy_route_versioned(
            cfg,
            deployment_id,
            &route_key,
            domain,
            primary.1,
            route_generation,
        )
        .await?;
    }
    let local_url = cfg.local_mode.then_some(domain);
    commit_candidate_activation(
        cfg,
        payload,
        deployment_id,
        route_generation,
        local_url,
        None,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn deploy_generated_topology(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
    app_id: Uuid,
    app_name: &str,
    route_key: &str,
    checkout: &Path,
    domain: &str,
    git_sync_duration_ms: u128,
) -> anyhow::Result<()> {
    let inventory = checkout_inventory(checkout).await?;
    let plan = hostlet_contracts::plan_repository_topology(&inventory);
    let config: GeneratedTopologyConfig = serde_json::from_value(
        payload
            .pointer("/runtime_config/generatedTopology")
            .cloned()
            .context("generated topology config is missing")?,
    )
    .context("generated topology config is invalid")?;
    hostlet_contracts::validate_generated_topology_config(&config).map_err(anyhow::Error::msg)?;
    let services = selected_services(&plan, &config)?;
    if services.is_empty() || services.len() > 2 {
        return topology_failure(
            cfg,
            deployment_id,
            "topology_selection_required",
            "Generated topology must contain one service or one frontend/backend pair.",
            &plan,
        )
        .await;
    }
    let lock_receipt = match repair_pnpm_lock_metadata(checkout, &inventory).await {
        Ok(receipt) => receipt,
        Err(err) => {
            let message = format!("Lockfile preflight failed: {err}");
            status_extra(
                cfg,
                deployment_id,
                "failed",
                StatusDetails {
                    failure: Some(&message),
                    failure_code: Some("lockfile_resolution_change"),
                    runtime_metadata: Some(json!({
                        "inferenceReceipt": inference_receipt(&plan, &services, &config, None),
                    })),
                    ..StatusDetails::default()
                },
            )
            .await;
            return Err(reported_deployment_failure(message));
        }
    };
    log_inference_plan(cfg, deployment_id, &plan, &services, lock_receipt.as_ref()).await;

    let public_origin = cfg.app_public_scheme.origin(domain);
    let public_ws_origin = cfg.app_public_scheme.websocket_origin(domain);
    let mut running: Vec<RunningService> = Vec::with_capacity(services.len());
    for service in &services {
        let service_slug = app_slug(&service.name);
        let image = format!("hostlet/{app_name}-{service_slug}:{deployment_id}");
        let container = format!("hostlet-{app_name}-{service_slug}-{deployment_id}");
        let mut service_payload = payload.clone();
        configure_service_payload(
            &mut service_payload,
            service,
            &public_origin,
            &public_ws_origin,
            checkout,
        )
        .await?;
        let built = build_image(
            cfg,
            deployment_id,
            &format!("{app_name}-{service_slug}"),
            &image,
            checkout,
            service.container_port.into(),
            &service_payload,
            git_sync_duration_ms,
        )
        .await?;
        status(cfg, deployment_id, "starting", None).await;
        let published_port = run_app_container(
            cfg,
            deployment_id,
            app_id,
            &image,
            &container,
            service.container_port.into(),
            built.hardening,
            &service_payload,
        )
        .await?;
        status(cfg, deployment_id, "health_checking", None).await;
        let health_result = match service.health_probe.kind {
            HealthProbeKind::Http => {
                let path = service.health_probe.path.as_deref().unwrap_or("/");
                wait_health(cfg, deployment_id, &container, published_port, path).await
            }
            HealthProbeKind::Tcp => {
                wait_tcp_health(cfg, deployment_id, &container, published_port).await
            }
        };
        if let Err(err) = health_result {
            for existing in &running {
                stop_failed_container_after_health_check(cfg, deployment_id, &existing.container)
                    .await;
            }
            stop_failed_container_after_health_check(cfg, deployment_id, &container).await;
            let failure = format!(
                "{} service readiness failed: {err}. All candidate services were stopped and the previous route was preserved.",
                service.name
            );
            status_extra(
                cfg,
                deployment_id,
                "failed",
                StatusDetails {
                    failure: Some(&failure),
                    failure_code: Some("topology_service_unhealthy"),
                    image: Some(&image),
                    container: Some(&container),
                    published_port: Some(published_port),
                    runtime_metadata: Some(json!({
                        "inferenceReceipt": inference_receipt(
                            &plan,
                            &services,
                            &config,
                            lock_receipt.as_ref(),
                        ),
                    })),
                    ..StatusDetails::default()
                },
            )
            .await;
            return Err(reported_deployment_failure(failure));
        }
        running.push(RunningService {
            service: service.clone(),
            image,
            container,
            published_port,
            runtime_metadata: built.runtime_metadata,
        });
    }

    let primary = running
        .iter()
        .find(|service| service.service.role == ServiceRole::Frontend)
        .or_else(|| running.first())
        .context("generated topology has no primary service")?;
    let backend = running
        .iter()
        .find(|service| service.service.role == ServiceRole::Backend);
    let service_reports = running.iter().map(service_report).collect::<Vec<_>>();
    let route_metadata = json!({
        "kind": if backend.is_some() { "split" } else { "single" },
        "frontendPort": primary.published_port,
        "backendPort": backend.map(|service| service.published_port),
        "backendPathPrefixes": config.backend_path_prefixes,
        "websocketsToBackend": backend.is_some(),
    });
    let runtime_metadata = json!({
        "runtime": "generated_topology",
        "inferenceReceipt": inference_receipt(
            &plan,
            &services,
            &config,
            lock_receipt.as_ref(),
        ),
        "routing": route_metadata,
        "serviceBuilds": running.iter().map(|service| json!({
            "selector": service.service.selector,
            "imageRef": service.image,
            "metadata": service.runtime_metadata,
        })).collect::<Vec<_>>(),
    });
    let route_generation = prepare_candidate_activation(
        cfg,
        payload,
        deployment_id,
        Some(&primary.image),
        &primary.container,
        primary.published_port,
        None,
        runtime_metadata.clone(),
        service_reports.clone(),
    )
    .await?;

    let routing_result = if let Some(backend) = backend {
        if cfg.local_mode {
            if let Some(router) = &cfg.local_router {
                apply_local_caddy_split_route_versioned(
                    cfg,
                    deployment_id,
                    router,
                    route_key,
                    domain,
                    primary.published_port,
                    backend.published_port,
                    &config.backend_path_prefixes,
                    route_generation,
                )
                .await
            } else {
                Ok(())
            }
        } else {
            apply_caddy_split_route_versioned(
                cfg,
                deployment_id,
                route_key,
                domain,
                primary.published_port,
                backend.published_port,
                &config.backend_path_prefixes,
                route_generation,
            )
            .await
        }
    } else if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route_versioned(
                cfg,
                deployment_id,
                router,
                route_key,
                domain,
                primary.published_port,
                route_generation,
            )
            .await
        } else {
            Ok(())
        }
    } else {
        apply_caddy_route_versioned(
            cfg,
            deployment_id,
            route_key,
            domain,
            primary.published_port,
            route_generation,
        )
        .await
    };
    if let Err(err) = routing_result {
        let failure = format!(
            "Generated topology routing failed: {err}. Candidate services remain available for recovery and the previous route was preserved when possible."
        );
        status_extra(
            cfg,
            deployment_id,
            "failed",
            StatusDetails {
                failure: Some(&failure),
                failure_code: Some("topology_routing_failed"),
                image: Some(&primary.image),
                container: Some(&primary.container),
                published_port: Some(primary.published_port),
                runtime_metadata: Some(runtime_metadata),
                services: Some(serde_json::to_value(service_reports).unwrap_or_default()),
                ..StatusDetails::default()
            },
        )
        .await;
        return Err(reported_deployment_failure(failure));
    }
    let local_url = cfg.local_mode.then(|| {
        if cfg.local_router.is_some() {
            domain.to_string()
        } else {
            format!("localhost:{}", primary.published_port)
        }
    });
    commit_candidate_activation(
        cfg,
        payload,
        deployment_id,
        route_generation,
        local_url.as_deref(),
        Some(&runtime_metadata),
        false,
    )
    .await?;
    status_extra(
        cfg,
        deployment_id,
        "success",
        StatusDetails {
            image: Some(&primary.image),
            container: Some(&primary.container),
            local_url: local_url.as_deref(),
            published_port: Some(primary.published_port),
            runtime_metadata: Some(runtime_metadata),
            services: Some(serde_json::to_value(service_reports).unwrap_or_default()),
            ..StatusDetails::default()
        },
    )
    .await;
    Ok(())
}

fn selected_services(
    plan: &hostlet_contracts::TopologyPlan,
    config: &GeneratedTopologyConfig,
) -> anyhow::Result<Vec<hostlet_contracts::InferredService>> {
    if config.mode == "auto" {
        if plan.readiness != TopologyReadiness::Ready {
            bail!("repository topology changed and now requires a service selection");
        }
        return Ok(plan.services.clone());
    }
    let mut selected = Vec::new();
    for (selector, role) in [
        (config.frontend_selector.as_deref(), ServiceRole::Frontend),
        (config.backend_selector.as_deref(), ServiceRole::Backend),
    ] {
        let Some(selector) = selector else {
            continue;
        };
        let candidate = plan
            .candidates
            .iter()
            .find(|candidate| candidate.selector == selector && candidate.role == role)
            .cloned()
            .with_context(|| format!("selected {role:?} service no longer exists"))?;
        selected.push(candidate);
    }
    Ok(selected)
}

async fn configure_service_payload(
    payload: &mut Value,
    service: &hostlet_contracts::InferredService,
    public_origin: &str,
    public_ws_origin: &str,
    checkout: &Path,
) -> anyhow::Result<()> {
    let object = payload
        .as_object_mut()
        .context("deploy payload must be an object")?;
    object.insert("container_port".into(), json!(service.container_port));
    object.insert("build_command".into(), json!(service.build_command));
    object.insert("start_command".into(), json!(service.start_command));
    object.insert("packaging_strategy".into(), json!("generated"));
    let config_name = format!(".hostlet-{}.railpack.json", service.role_string());
    let config_path = checkout.join(&config_name);
    tokio::fs::write(
        &config_path,
        serde_json::to_vec(&json!({"provider": service.provider}))?,
    )
    .await?;
    object.insert("_hostlet_railpack_config".into(), json!(config_name));
    let original_env = object
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut runtime_env =
        if service.role == ServiceRole::Frontend && service.output_directory.is_some() {
            serde_json::Map::new()
        } else {
            original_env.clone()
        };
    let mut build_env = serde_json::Map::new();
    if let Some(output) = service.output_directory.as_deref() {
        build_env.insert("RAILPACK_SPA_OUTPUT_DIR".into(), json!(output));
    }
    for key in &service.public_env {
        let value = original_env.get(key).cloned().unwrap_or_else(|| {
            if key.contains("WS_URL") {
                json!(public_ws_origin)
            } else {
                json!(public_origin)
            }
        });
        runtime_env.insert(key.clone(), value.clone());
        build_env.insert(key.clone(), value);
    }
    object.insert("env".into(), Value::Object(runtime_env));
    object.insert("_hostlet_build_env".into(), Value::Object(build_env));
    Ok(())
}

trait ServiceRoleLabel {
    fn role_string(&self) -> &'static str;
}

impl ServiceRoleLabel for hostlet_contracts::InferredService {
    fn role_string(&self) -> &'static str {
        match self.role {
            ServiceRole::Frontend => "frontend",
            ServiceRole::Backend => "backend",
            ServiceRole::Web => "web",
        }
    }
}

fn service_report(service: &RunningService) -> hostlet_contracts::DeploymentServiceReport {
    hostlet_contracts::DeploymentServiceReport {
        name: service.service.name.clone(),
        // deployment_services predates inferred topologies and deliberately keeps
        // the stable web/backing role vocabulary. Both public-facing topology
        // services are web services; the inference receipt preserves the more
        // specific frontend/backend role used for routing.
        role: "web".to_string(),
        container_name: Some(service.container.clone()),
        image_tag: Some(service.image.clone()),
        target_port: Some(service.service.container_port.into()),
        published_port: Some(service.published_port.into()),
        status: Some("running".to_string()),
        health_status: Some("healthy".to_string()),
    }
}

async fn topology_failure(
    cfg: &Config,
    deployment_id: Uuid,
    code: &str,
    failure: &str,
    plan: &hostlet_contracts::TopologyPlan,
) -> anyhow::Result<()> {
    status_extra(
        cfg,
        deployment_id,
        "failed",
        StatusDetails {
            failure: Some(failure),
            failure_code: Some(code),
            runtime_metadata: Some(json!({"inferencePlan": plan})),
            ..StatusDetails::default()
        },
    )
    .await;
    Err(reported_deployment_failure(failure.to_string()))
}

async fn log_inference_plan(
    cfg: &Config,
    deployment_id: Uuid,
    plan: &hostlet_contracts::TopologyPlan,
    services: &[hostlet_contracts::InferredService],
    lock_receipt: Option<&Value>,
) {
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!(
            "Hostlet inference plan v{} ({:?} confidence): {}",
            plan.schema_version, plan.confidence, plan.summary
        ),
    )
    .await;
    for service in services {
        log(
            cfg,
            deployment_id,
            "stdout",
            &format!(
                "Inferred {:?} {} at {}: build={}, start={}, health={:?}{}",
                service.role,
                service.name,
                service.root_directory,
                service.build_command.as_deref().unwrap_or("none"),
                service
                    .start_command
                    .as_deref()
                    .unwrap_or("Railpack static runtime"),
                service.health_probe.kind,
                service
                    .health_probe
                    .path
                    .as_deref()
                    .map(|path| format!(" {path}"))
                    .unwrap_or_default(),
            ),
        )
        .await;
    }
    if let Some(receipt) = lock_receipt {
        log(
            cfg,
            deployment_id,
            "stdout",
            &format!(
                "Safely repaired {} direct lockfile specifier metadata entries; resolved graph unchanged.",
                receipt.get("changedSpecifiers").and_then(Value::as_u64).unwrap_or_default()
            ),
        )
        .await;
    }
}

fn inference_receipt(
    plan: &hostlet_contracts::TopologyPlan,
    services: &[hostlet_contracts::InferredService],
    config: &GeneratedTopologyConfig,
    lock_receipt: Option<&Value>,
) -> Value {
    json!({
        "schemaVersion": plan.schema_version,
        "confidence": plan.confidence,
        "mode": config.mode,
        "services": services,
        "routing": {
            "websocketsToBackend": services.iter().any(|service| service.role == ServiceRole::Backend),
            "backendPathPrefixes": config.backend_path_prefixes,
        },
        "lockfile": lock_receipt,
        "repositoryModified": false,
    })
}

async fn checkout_inventory(checkout: &Path) -> anyhow::Result<RepositoryInventory> {
    let mut stack = vec![checkout.to_path_buf()];
    let mut files = Vec::new();
    let mut visited = 0usize;
    let mut content_bytes = 0usize;
    while let Some(directory) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            visited += 1;
            if visited > INVENTORY_MAX_FILES {
                bail!("repository inventory exceeds {INVENTORY_MAX_FILES} entries");
            }
            let file_type = entry.file_type().await?;
            let path = entry.path();
            let relative = path
                .strip_prefix(checkout)
                .context("inventory path escaped checkout")?
                .to_string_lossy()
                .replace('\\', "/");
            if file_type.is_dir() {
                if !noise_directory(entry.file_name().to_string_lossy().as_ref()) {
                    stack.push(path);
                }
                continue;
            }
            if !file_type.is_file() || !inventory_path(&relative) {
                continue;
            }
            if files.len() >= INVENTORY_MAX_RELEVANT_FILES {
                bail!("repository has too many topology-relevant files");
            }
            let metadata = entry.metadata().await?;
            let is_lock = lock_filename(&relative);
            let contents = if is_lock
                || metadata.len() > 128 * 1024
                || content_bytes + metadata.len() as usize > INVENTORY_MAX_CONTENT_BYTES
            {
                None
            } else {
                let contents = tokio::fs::read_to_string(&path).await.ok();
                content_bytes += contents.as_ref().map(String::len).unwrap_or_default();
                contents
            };
            files.push(RepositoryFile {
                path: relative,
                contents,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(RepositoryInventory { files })
}

fn noise_directory(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
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
        )
}

fn inventory_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    matches!(
        filename,
        "package.json"
            | "pnpm-workspace.yaml"
            | "pnpm-lock.yaml"
            | "package-lock.json"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "go.work"
            | "Cargo.toml"
            | "index.html"
            | "main.rs"
    ) || path.ends_with(".go")
        || matches!(
            path.rsplit('.').next(),
            Some("js" | "jsx" | "ts" | "tsx" | "vue" | "svelte")
        )
}

fn lock_filename(path: &str) -> bool {
    matches!(
        path.rsplit('/').next(),
        Some("pnpm-lock.yaml" | "package-lock.json" | "yarn.lock" | "bun.lock" | "bun.lockb")
    )
}

async fn repair_pnpm_lock_metadata(
    checkout: &Path,
    inventory: &RepositoryInventory,
) -> anyhow::Result<Option<Value>> {
    let lock_path = checkout.join("pnpm-lock.yaml");
    if !tokio::fs::try_exists(&lock_path).await? {
        return Ok(None);
    }
    let before = tokio::fs::read(&lock_path).await?;
    let mut lock: serde_yaml::Value =
        serde_yaml::from_slice(&before).context("pnpm-lock.yaml is not valid YAML")?;
    let importers = lock
        .get_mut("importers")
        .and_then(serde_yaml::Value::as_mapping_mut)
        .context("pnpm-lock.yaml has no importers map")?;
    let mut changed = 0usize;
    for manifest in inventory
        .files
        .iter()
        .filter(|file| file.path.ends_with("package.json"))
    {
        let Some(contents) = manifest.contents.as_deref() else {
            continue;
        };
        let package: Value = serde_json::from_str(contents)
            .with_context(|| format!("{} is invalid JSON", manifest.path))?;
        let importer_key = manifest.path.strip_suffix("/package.json").unwrap_or(".");
        let importer_key = if manifest.path == "package.json" {
            "."
        } else {
            importer_key
        };
        let Some(importer) = importers
            .get_mut(serde_yaml::Value::String(importer_key.to_string()))
            .and_then(serde_yaml::Value::as_mapping_mut)
        else {
            bail!("pnpm lockfile is missing importer {importer_key}");
        };
        for section in ["dependencies", "devDependencies", "optionalDependencies"] {
            let Some(declared) = package.get(section).and_then(Value::as_object) else {
                continue;
            };
            let Some(locked_section) = importer
                .get_mut(serde_yaml::Value::String(section.to_string()))
                .and_then(serde_yaml::Value::as_mapping_mut)
            else {
                if !declared.is_empty() {
                    bail!("pnpm lockfile importer {importer_key} is missing {section}");
                }
                continue;
            };
            for (name, declared_specifier) in declared {
                let declared_specifier = declared_specifier
                    .as_str()
                    .context("package dependency specifier must be a string")?;
                let Some(locked) = locked_section
                    .get_mut(serde_yaml::Value::String(name.clone()))
                    .and_then(serde_yaml::Value::as_mapping_mut)
                else {
                    bail!("pnpm lockfile importer {importer_key} is missing {name}");
                };
                let locked_specifier = locked
                    .get(serde_yaml::Value::String("specifier".to_string()))
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or_default();
                if locked_specifier == declared_specifier {
                    continue;
                }
                let resolved = locked
                    .get(serde_yaml::Value::String("version".to_string()))
                    .and_then(serde_yaml::Value::as_str)
                    .context("pnpm lock dependency has no resolved version")?;
                if !resolved_satisfies(declared_specifier, resolved) {
                    bail!(
                        "{manifest_path} requests {name}@{declared_specifier}, but the lockfile resolves {resolved}; run pnpm install and commit the updated lockfile",
                        manifest_path = manifest.path
                    );
                }
                locked.insert(
                    serde_yaml::Value::String("specifier".to_string()),
                    serde_yaml::Value::String(declared_specifier.to_string()),
                );
                changed += 1;
            }
        }
    }
    if changed == 0 {
        return Ok(None);
    }
    let graph_before = pnpm_graph_fingerprint(&serde_yaml::from_slice(&before)?);
    let after = serde_yaml::to_string(&lock)?.into_bytes();
    let graph_after = pnpm_graph_fingerprint(&lock);
    if graph_before != graph_after {
        bail!("pnpm lock repair would change the resolved dependency graph");
    }
    tokio::fs::write(&lock_path, &after).await?;
    Ok(Some(json!({
        "manager": "pnpm",
        "status": "metadata_repaired",
        "changedSpecifiers": changed,
        "beforeSha256": sha256_hex(&before),
        "afterSha256": sha256_hex(&after),
        "resolvedGraphUnchanged": true,
        "ephemeral": true,
    })))
}

fn resolved_satisfies(specifier: &str, resolved: &str) -> bool {
    if specifier.starts_with("workspace:")
        || specifier.starts_with("file:")
        || specifier.starts_with("link:")
        || specifier.starts_with("git")
    {
        return specifier == resolved;
    }
    let version = resolved
        .split(['(', '_'])
        .next()
        .and_then(|value| Version::parse(value.trim_start_matches('v')).ok());
    let requirement = VersionReq::parse(specifier).ok();
    matches!((requirement, version), (Some(requirement), Some(version)) if requirement.matches(&version))
}

fn pnpm_graph_fingerprint(lock: &serde_yaml::Value) -> String {
    let value = json!({
        "packages": lock.get("packages"),
        "snapshots": lock.get("snapshots"),
        "importerVersions": lock.get("importers").and_then(serde_yaml::Value::as_mapping).map(|importers| {
            importers.iter().map(|(key, importer)| {
                let versions = importer.as_mapping().map(|sections| {
                    sections.iter().filter_map(|(section, values)| {
                        let section = section.as_str()?;
                        matches!(section, "dependencies" | "devDependencies" | "optionalDependencies").then(|| {
                            let values = values.as_mapping().map(|values| values.iter().filter_map(|(name, value)| {
                                Some((name.as_str()?.to_string(), value.get("version").cloned()))
                            }).collect::<BTreeMap<_, _>>()).unwrap_or_default();
                            (section.to_string(), values)
                        })
                    }).collect::<BTreeMap<_, _>>()
                }).unwrap_or_default();
                (key.as_str().unwrap_or_default().to_string(), versions)
            }).collect::<BTreeMap<_, _>>()
        }),
    });
    sha256_hex(value.to_string().as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "generated_topology/tests.rs"]
mod tests;
