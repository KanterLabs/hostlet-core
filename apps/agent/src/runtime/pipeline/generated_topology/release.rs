struct RunningService {
    service: hostlet_contracts::InferredService,
    image: String,
    container: String,
    published_port: u16,
    runtime_metadata: Value,
}

pub(crate) async fn release_generated_topology(
    cfg: &Config,
    payload: &Value,
    release_services: Vec<hostlet_contracts::ReleaseService>,
    inferred_services: Vec<hostlet_contracts::InferredService>,
    config: GeneratedTopologyConfig,
    inference_receipt: Value,
) -> anyhow::Result<()> {
    use hostlet_contracts::{ContainerFilesystemMode, RuntimeEnvPolicy};

    anyhow::ensure!(
        !release_services.is_empty()
            && release_services.len() <= 2
            && release_services.len() == inferred_services.len(),
        "release topology has an invalid service set"
    );
    let deployment_id =
        Uuid::parse_str(payload["deployment_id"].as_str().context("deployment_id")?)?;
    let app_id = Uuid::parse_str(payload["app_id"].as_str().context("app_id")?)?;
    let app_name = app_slug(&format!("app-{app_id}"));
    let route_key = payload
        .get("route_key")
        .and_then(Value::as_str)
        .map(app_slug)
        .unwrap_or_else(|| app_name.clone());
    let domain = payload["domain"].as_str().context("domain")?;
    validate_domain(domain)?;
    let public_origin = cfg.app_public_scheme.origin(domain);
    let public_ws_origin = cfg.app_public_scheme.websocket_origin(domain);
    let original_env = payload
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut running: Vec<RunningService> = Vec::with_capacity(release_services.len());
    for service in release_services {
        let inferred = inferred_services
            .iter()
            .find(|candidate| candidate.name == service.name)
            .cloned()
            .with_context(|| {
                format!(
                    "release service {} is not in inference receipt",
                    service.name
                )
            })?;
        anyhow::ensure!(
            inferred.container_port == service.container_port,
            "release service port does not match inference receipt"
        );
        let mut service_payload = payload.clone();
        let object = service_payload
            .as_object_mut()
            .context("release payload must be an object")?;
        object.insert("container_port".into(), json!(service.container_port));
        let mut runtime_env = match service.env_policy {
            RuntimeEnvPolicy::All => original_env.clone(),
            RuntimeEnvPolicy::None => serde_json::Map::new(),
        };
        for key in &service.public_env {
            let value = original_env.get(key).cloned().unwrap_or_else(|| {
                if key.contains("WS_URL") {
                    json!(public_ws_origin)
                } else {
                    json!(public_origin)
                }
            });
            runtime_env.insert(key.clone(), value);
        }
        object.insert("env".into(), Value::Object(runtime_env));
        let service_slug = app_slug(&service.name);
        let container = format!("hostlet-{app_name}-{service_slug}-{deployment_id}");
        let hardening = match service.filesystem {
            ContainerFilesystemMode::ReadOnly => ContainerHardening::ReadOnlyRootFs,
            ContainerFilesystemMode::Writable => ContainerHardening::WritableRootFs,
        };
        status(cfg, deployment_id, "starting", None).await;
        let published_port = run_app_container(
            cfg,
            deployment_id,
            app_id,
            &service.image_digest_ref,
            &container,
            service.container_port.into(),
            hardening,
            &service_payload,
        )
        .await?;
        status(cfg, deployment_id, "health_checking", None).await;
        let health_result = match service.health_probe.kind {
            HealthProbeKind::Http => {
                wait_health(
                    cfg,
                    deployment_id,
                    &container,
                    published_port,
                    service.health_probe.path.as_deref().unwrap_or("/"),
                )
                .await
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
                    image: Some(&service.image_digest_ref),
                    container: Some(&container),
                    published_port: Some(published_port),
                    runtime_metadata: Some(json!({"inferenceReceipt": inference_receipt})),
                    ..StatusDetails::default()
                },
            )
            .await;
            return Err(reported_deployment_failure(failure));
        }
        running.push(RunningService {
            service: inferred,
            image: service.image_digest_ref,
            container,
            published_port,
            runtime_metadata: service.runtime_metadata,
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
    let runtime_metadata = json!({
        "runtime": "generated_topology",
        "inferenceReceipt": inference_receipt,
        "routing": {
            "kind": if backend.is_some() { "split" } else { "single" },
            "frontendPort": primary.published_port,
            "backendPort": backend.map(|service| service.published_port),
            "backendPathPrefixes": config.backend_path_prefixes,
            "websocketsToBackend": backend.is_some(),
        },
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
                    &route_key,
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
                &route_key,
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
                &route_key,
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
            &route_key,
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
        payload.get("type").and_then(Value::as_str) == Some("rollback"),
    )
    .await?;
    if payload.get("type").and_then(Value::as_str) != Some("rollback") {
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
    }
    Ok(())
}
