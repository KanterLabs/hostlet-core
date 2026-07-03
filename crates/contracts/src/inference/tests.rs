use super::*;

fn set(items: &[&str]) -> std::collections::HashSet<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn start_command_prefers_procfile_web_process() {
    let suggestion = detect_start_command(RepoCommandFiles {
        procfile: Some("worker: node worker.js\nweb: node server.js\n"),
        package_json: Some(r#"{"scripts":{"start":"next start"}}"#),
        ..RepoCommandFiles::default()
    })
    .unwrap();

    assert_eq!(suggestion.command, "node server.js");
    assert_eq!(suggestion.source_file, "Procfile");
    assert_eq!(suggestion.source_detail, "web");
    assert_eq!(suggestion.confidence, "high");
}

#[test]
fn package_json_start_command_uses_package_manager_script() {
    let suggestion = detect_start_command(RepoCommandFiles {
        package_json: Some(r#"{"packageManager":"pnpm@10.0.0","scripts":{"start":"next start"}}"#),
        ..RepoCommandFiles::default()
    })
    .unwrap();

    assert_eq!(suggestion.command, "pnpm run start");
    assert_eq!(suggestion.source_file, "package.json");
    assert_eq!(suggestion.source_detail, "scripts.start");
    assert_eq!(suggestion.confidence, "high");
}

#[test]
fn package_json_preview_is_medium_confidence_fallback() {
    let suggestion = detect_start_command(RepoCommandFiles {
        package_json: Some(r#"{"scripts":{"preview":"vite preview --host 0.0.0.0"}}"#),
        ..RepoCommandFiles::default()
    })
    .unwrap();

    assert_eq!(suggestion.command, "npm run preview");
    assert_eq!(suggestion.source_detail, "scripts.preview");
    assert_eq!(suggestion.confidence, "medium");
}

#[test]
fn railway_and_render_configs_can_supply_start_commands() {
    let railway = detect_start_command(RepoCommandFiles {
        railway_json: Some(r#"{"deploy":{"startCommand":"python main.py"}}"#),
        ..RepoCommandFiles::default()
    })
    .unwrap();
    assert_eq!(railway.command, "python main.py");
    assert_eq!(railway.source_file, "railway.json");
    assert_eq!(railway.source_detail, "deploy.startCommand");

    let render = detect_start_command(RepoCommandFiles {
        render_yaml: Some((
            "render.yml",
            "services:\n  - type: web\n    startCommand: gunicorn app:app\n",
        )),
        ..RepoCommandFiles::default()
    })
    .unwrap();
    assert_eq!(render.command, "gunicorn app:app");
    assert_eq!(render.source_file, "render.yml");
    assert_eq!(render.source_detail, "services[0].startCommand");
}

#[test]
fn dockerfile_cmd_is_low_confidence_fallback() {
    let suggestion = detect_start_command(RepoCommandFiles {
        dockerfile: Some("FROM node:22\nCMD [\"node\", \"server.js\"]\n"),
        ..RepoCommandFiles::default()
    })
    .unwrap();

    assert_eq!(suggestion.command, "node server.js");
    assert_eq!(suggestion.source_file, "Dockerfile");
    assert_eq!(suggestion.source_detail, "CMD");
    assert_eq!(suggestion.confidence, "low");
}

#[test]
fn invalid_detected_commands_are_ignored() {
    let suggestion = detect_start_command(RepoCommandFiles {
        package_json: Some(r#"{"scripts":{"start":"node server.js\nrm -rf /"}}"#),
        ..RepoCommandFiles::default()
    });

    assert!(suggestion.is_none());
}

#[test]
fn package_json_dependencies_lowercases_deps_and_dev_deps() {
    let deps = package_json_dependencies(
        r#"{"dependencies":{"PG":"^8","ioredis":"^5"},"devDependencies":{"Vitest":"^1"}}"#,
    );
    assert!(deps.contains("pg"));
    assert!(deps.contains("ioredis"));
    assert!(deps.contains("vitest"));
}

#[test]
fn node_postgres_and_redis_deps_map_to_catalog_addons() {
    let detected = infer_service_addons(&set(&["pg", "ioredis", "react"]));
    // Catalog order: postgres before redis.
    assert_eq!(detected.addons, vec!["postgres", "redis"]);
    assert!(detected.warnings.is_empty());
}

#[test]
fn no_data_deps_detect_nothing() {
    let detected = infer_service_addons(&set(&["react", "next", "lodash"]));
    assert!(detected.addons.is_empty());
    assert!(detected.warnings.is_empty());
}

#[test]
fn bare_pg_matches_only_exactly_not_as_substring() {
    // `pg` is exact-match: a package that merely contains "pg" must not trip it.
    assert!(infer_service_addons(&set(&["imagepg-tools"]))
        .addons
        .is_empty());
    assert_eq!(infer_service_addons(&set(&["pg"])).addons, vec!["postgres"]);
}

#[test]
fn orm_dependency_infers_postgres_default() {
    assert_eq!(
        infer_service_addons(&set(&["prisma"])).addons,
        vec!["postgres"]
    );
    assert_eq!(
        infer_service_addons(&set(&["@prisma/client"])).addons,
        vec!["postgres"]
    );
}

#[test]
fn unsupported_service_is_skipped_with_a_warning_not_an_addon() {
    let detected = infer_service_addons(&set(&["mongoose"]));
    assert!(detected.addons.is_empty());
    assert_eq!(detected.warnings.len(), 1);
    assert!(detected.warnings[0].contains("mongodb"));
}

#[test]
fn python_manifest_tokens_detect_postgres_and_redis() {
    let tokens = manifest_dependency_tokens("psycopg2-binary==2.9.9\nredis>=5.0\nflask\n");
    let detected = infer_service_addons(&tokens);
    assert_eq!(detected.addons, vec!["postgres", "redis"]);
}

#[test]
fn go_mod_module_paths_detect_postgres() {
    let tokens = manifest_dependency_tokens(
        "module example.com/app\n\nrequire (\n\tgithub.com/lib/pq v1.10.9\n)\n",
    );
    assert_eq!(infer_service_addons(&tokens).addons, vec!["postgres"]);
}

#[test]
fn cargo_toml_crate_names_detect_postgres() {
    let tokens =
        manifest_dependency_tokens("[dependencies]\ntokio-postgres = \"0.7\"\nserde = \"1\"\n");
    assert_eq!(infer_service_addons(&tokens).addons, vec!["postgres"]);
}

#[test]
fn compose_service_images_map_to_catalog_addons() {
    let compose = "\
services:
  api:
    build: .
  db:
    image: postgres:16-alpine
  cache:
    image: redis:7-alpine
  search:
    image: elasticsearch:8.0
";
    let detected = infer_addons_from_compose(compose);
    // postgres + redis map to the catalog; elasticsearch has no catalog
    // entry and no signal, so it is silently ignored (not warned).
    assert_eq!(detected.addons, vec!["postgres", "redis"]);
}

#[test]
fn merge_unions_and_keeps_catalog_order() {
    let mut a = infer_service_addons(&set(&["ioredis"]));
    let b = infer_service_addons(&set(&["pg"]));
    a.merge(&b);
    assert_eq!(a.addons, vec!["postgres", "redis"]);
}

#[test]
fn with_detected_services_flips_node_app_to_compose_runtime() {
    let base = node_inspection(
        "owner/shop-api",
        "main",
        "main",
        PackageInference {
            framework: "Next.js",
            package_manager: "pnpm",
        },
        false,
    );
    let detected = infer_service_addons(&set(&["pg", "ioredis"]));
    let value = with_detected_services(base, &detected);

    assert_eq!(value["runtimeKind"], "compose");
    assert_eq!(value["webService"], "web");
    assert_eq!(value["deployable"], true);
    // Detected framework metadata is preserved through the overlay.
    assert_eq!(value["detectedFramework"], "Next.js");
    // The create handler resolves these into a generated compose stack.
    assert_eq!(
        value.pointer("/runtimeConfig/compose/addOns"),
        Some(&serde_json::json!([{"key":"postgres"},{"key":"redis"}]))
    );
    // Preview: the repo-built web service plus the managed backing services.
    let services = value["services"].as_array().unwrap();
    assert_eq!(services.len(), 3);
    assert_eq!(services[0]["role"], "web");
    assert_eq!(services[0]["build"], true);
    assert!(services
        .iter()
        .any(|s| s["name"] == "postgres" && s["role"] == "backing"));
}

#[test]
fn with_detected_services_leaves_single_apps_single_but_surfaces_skip_notes() {
    let base = node_inspection(
        "owner/app",
        "main",
        "main",
        PackageInference {
            framework: "Node",
            package_manager: "npm",
        },
        false,
    );
    let detected = infer_service_addons(&set(&["mongoose"]));
    let value = with_detected_services(base, &detected);

    // No managed add-on → still a single-runtime app, but the user is told
    // their Mongo dependency was skipped.
    assert_eq!(value["runtimeKind"], "single");
    assert!(value.get("services").is_none());
    assert!(value["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("mongodb")));
}
