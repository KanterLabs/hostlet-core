use super::*;
use crate::{
    repository_inventory_entry_count_within_bound, repository_inventory_file_content_within_bound,
    repository_inventory_relevant_file_count_within_bound,
    repository_inventory_response_is_complete, repository_inventory_select_candidates,
    RepositoryInventoryCandidate, REPOSITORY_INVENTORY_MAX_CONTENT_BYTES,
    REPOSITORY_INVENTORY_MAX_ENTRIES, REPOSITORY_INVENTORY_MAX_FILE_BYTES,
    REPOSITORY_INVENTORY_MAX_RELEVANT_FILES,
};

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
fn repository_inventory_bounds_accept_exact_and_reject_overflow() {
    assert!(repository_inventory_entry_count_within_bound(
        REPOSITORY_INVENTORY_MAX_ENTRIES
    ));
    assert!(!repository_inventory_entry_count_within_bound(
        REPOSITORY_INVENTORY_MAX_ENTRIES + 1
    ));
    assert!(repository_inventory_relevant_file_count_within_bound(
        REPOSITORY_INVENTORY_MAX_RELEVANT_FILES
    ));
    assert!(!repository_inventory_relevant_file_count_within_bound(
        REPOSITORY_INVENTORY_MAX_RELEVANT_FILES + 1
    ));
}

#[test]
fn repository_inventory_provider_and_content_bounds_match_both_builders() {
    assert!(repository_inventory_response_is_complete(false));
    assert!(!repository_inventory_response_is_complete(true));
    assert!(repository_inventory_file_content_within_bound(
        0,
        REPOSITORY_INVENTORY_MAX_FILE_BYTES
    ));
    assert!(!repository_inventory_file_content_within_bound(
        0,
        REPOSITORY_INVENTORY_MAX_FILE_BYTES + 1
    ));
    assert!(repository_inventory_file_content_within_bound(
        REPOSITORY_INVENTORY_MAX_CONTENT_BYTES - REPOSITORY_INVENTORY_MAX_FILE_BYTES,
        REPOSITORY_INVENTORY_MAX_FILE_BYTES
    ));
    assert!(!repository_inventory_file_content_within_bound(
        REPOSITORY_INVENTORY_MAX_CONTENT_BYTES - REPOSITORY_INVENTORY_MAX_FILE_BYTES + 1,
        REPOSITORY_INVENTORY_MAX_FILE_BYTES
    ));
}

#[test]
fn inventory_selection_preserves_sorted_files_for_the_same_topology_plan() {
    let selected = repository_inventory_select_candidates(vec![
        RepositoryInventoryCandidate::new("z.js", REPOSITORY_INVENTORY_MAX_FILE_BYTES),
        RepositoryInventoryCandidate::new("a.js", REPOSITORY_INVENTORY_MAX_CONTENT_BYTES),
        RepositoryInventoryCandidate::new("pnpm-lock.yaml", REPOSITORY_INVENTORY_MAX_FILE_BYTES),
        RepositoryInventoryCandidate::new("package.json", 64),
    ])
    .unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|file| (file.path.as_str(), file.include_contents))
            .collect::<Vec<_>>(),
        vec![
            ("a.js", false),
            ("package.json", true),
            ("pnpm-lock.yaml", false),
            ("z.js", true),
        ]
    );

    let inventory = RepositoryInventory {
        files: selected
            .into_iter()
            .map(|file| RepositoryFile {
                path: file.path.clone(),
                contents: (file.path == "package.json").then(|| {
                    r#"{"scripts":{"start":"node index.js"},"dependencies":{"express":"1"}}"#
                        .to_string()
                }),
            })
            .collect(),
    };
    let plan = plan_repository_topology(&inventory);
    assert_eq!(plan.readiness, TopologyReadiness::Ready);
    assert_eq!(plan.services[0].provider, "node");
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
