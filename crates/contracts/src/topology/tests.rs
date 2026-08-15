use super::*;

fn inventory(files: &[(&str, &str)]) -> RepositoryInventory {
    RepositoryInventory {
        files: files
            .iter()
            .map(|(path, contents)| RepositoryFile {
                path: (*path).to_string(),
                contents: Some((*contents).to_string()),
            })
            .collect(),
    }
}

#[test]
fn competing_backends_require_selection() {
    let plan = plan_repository_topology(&inventory(&[
        ("package.json", "{}"),
        (
            "apps/one/package.json",
            r#"{"name":"one","scripts":{"start":"node one.js"},"dependencies":{"express":"1"}}"#,
        ),
        (
            "apps/two/package.json",
            r#"{"name":"two","scripts":{"start":"node two.js"},"dependencies":{"fastify":"1"}}"#,
        ),
    ]));
    assert_eq!(plan.readiness, TopologyReadiness::NeedsSelection);
    assert!(plan.services.is_empty());
    assert_eq!(plan.candidates.len(), 2);
}

#[test]
fn noise_directories_do_not_become_candidates() {
    let plan = plan_repository_topology(&inventory(&[(
        "examples/demo/package.json",
        r#"{"scripts":{"start":"node index.js"},"dependencies":{"express":"1"}}"#,
    )]));
    assert_eq!(plan.readiness, TopologyReadiness::Unsupported);
}

#[test]
fn selected_config_requires_safe_prefixes_and_selector() {
    let mut config = GeneratedTopologyConfig {
        mode: "selected".to_string(),
        ..GeneratedTopologyConfig::default()
    };
    assert!(validate_generated_topology_config(&config).is_err());
    config.backend_selector = Some("node:apps/api/package.json:api".to_string());
    assert!(validate_generated_topology_config(&config).is_ok());
    config.backend_path_prefixes = vec!["/api/*".to_string()];
    assert!(validate_generated_topology_config(&config).is_err());
}

#[test]
fn workspace_commands_follow_the_detected_package_manager() {
    for (manager_file, manager_contents, expected) in [
        ("package-lock.json", "{}", "npm run start --workspace api"),
        ("yarn.lock", "", "yarn workspace api run start"),
        ("bun.lock", "", "bun run --filter api start"),
        (
            "pnpm-lock.yaml",
            "lockfileVersion: '9.0'",
            "pnpm --filter api run start",
        ),
    ] {
        let plan = plan_repository_topology(&inventory(&[
            (manager_file, manager_contents),
            (
                "apps/api/package.json",
                r#"{"name":"api","scripts":{"start":"node index.js"},"dependencies":{"express":"1"}}"#,
            ),
        ]));
        assert_eq!(plan.readiness, TopologyReadiness::Ready, "{manager_file}");
        assert_eq!(
            plan.services[0].start_command.as_deref(),
            Some(expected),
            "{manager_file}"
        );
    }
}

#[test]
fn supported_non_node_runtimes_are_inferred() {
    let cases: &[(&[(&str, &str)], &str)] = &[
        (
            &[
                ("service/requirements.txt", "fastapi\nuvicorn"),
                ("service/main.py", "app = 1"),
            ],
            "python",
        ),
        (
            &[
                ("service/go.mod", "module example.test/service"),
                ("service/main.go", "package main\nfunc main() {}"),
            ],
            "golang",
        ),
        (
            &[
                (
                    "service/Cargo.toml",
                    "[package]\nname = \"service\"\nversion = \"0.1.0\"",
                ),
                ("service/src/main.rs", "fn main() {}"),
            ],
            "rust",
        ),
        (&[("site/index.html", "<h1>ok</h1>")], "staticfile"),
    ];
    for (files, provider) in cases {
        let plan = plan_repository_topology(&inventory(files));
        assert_eq!(plan.readiness, TopologyReadiness::Ready, "{provider}");
        assert_eq!(plan.services[0].provider, *provider);
    }
}

#[test]
fn attach_plan_adds_auto_selection_without_addons() {
    let plan = plan_repository_topology(&inventory(&[(
        "package.json",
        r#"{"scripts":{"start":"node index.js"},"dependencies":{"express":"1"}}"#,
    )]));
    let inspection = attach_topology_plan(serde_json::json!({}), &plan);
    assert_eq!(inspection["deployable"], true);
    assert_eq!(
        inspection["runtimeConfig"]["generatedTopology"]["mode"],
        "auto"
    );
}

#[test]
fn attach_plan_rejects_managed_addons_with_generated_topology() {
    let plan = plan_repository_topology(&inventory(&[(
        "package.json",
        r#"{"scripts":{"start":"node index.js"},"dependencies":{"express":"1"}}"#,
    )]));
    let inspection = attach_topology_plan(
        serde_json::json!({"runtimeConfig":{"compose":{"addOns":[{"key":"postgres"}]}}}),
        &plan,
    );
    assert_eq!(inspection["deployable"], false);
    assert_eq!(
        inspection["runtimeConfig"]["generatedTopology"]["mode"],
        "auto"
    );
    assert_eq!(
        inspection["runtimeConfig"]["compose"]["addOns"][0]["key"],
        "postgres"
    );
    assert!(inspection["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .is_some_and(|warning| warning.contains("cannot be combined"))));
}

#[test]
fn attach_plan_warns_before_selection_when_managed_addons_are_detected() {
    let plan = plan_repository_topology(&inventory(&[
        ("package.json", "{}"),
        (
            "apps/one/package.json",
            r#"{"name":"one","scripts":{"start":"node one.js"},"dependencies":{"express":"1"}}"#,
        ),
        (
            "apps/two/package.json",
            r#"{"name":"two","scripts":{"start":"node two.js"},"dependencies":{"fastify":"1"}}"#,
        ),
    ]));
    let inspection = attach_topology_plan(
        serde_json::json!({"runtimeConfig":{"compose":{"addOns":[{"key":"postgres"},{"key":"redis"}]}}}),
        &plan,
    );
    assert_eq!(plan.readiness, TopologyReadiness::NeedsSelection);
    assert_eq!(inspection["deployable"], false);
    assert!(inspection
        .pointer("/runtimeConfig/generatedTopology")
        .is_none());
    assert!(inspection["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .is_some_and(|warning| warning.contains("cannot be combined"))));
}
