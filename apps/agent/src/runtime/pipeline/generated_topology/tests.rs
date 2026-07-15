use super::*;

fn temp_checkout(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hostlet-topology-{label}-{}", Uuid::new_v4()))
}

fn patchwork_inventory(client_manifest: &str) -> RepositoryInventory {
    RepositoryInventory {
        files: vec![RepositoryFile {
            path: "packages/client/package.json".into(),
            contents: Some(client_manifest.into()),
        }],
    }
}

#[test]
fn semver_proof_accepts_patchwork_metadata_only_change() {
    assert!(resolved_satisfies("^1.61.1", "1.61.1"));
    assert!(!resolved_satisfies("^2.0.0", "1.61.1"));
}

#[tokio::test]
async fn public_endpoint_values_are_injected_into_frontend_build_only() {
    let checkout = temp_checkout("public-env");
    tokio::fs::create_dir_all(&checkout).await.unwrap();
    let service = hostlet_contracts::InferredService {
        selector: "node:client/package.json:client".into(),
        name: "client".into(),
        role: ServiceRole::Frontend,
        root_directory: "client".into(),
        provider: "node".into(),
        package_manager: Some("pnpm".into()),
        build_command: Some("pnpm build".into()),
        start_command: None,
        output_directory: Some("client/dist".into()),
        container_port: 80,
        health_probe: hostlet_contracts::HealthProbe {
            kind: HealthProbeKind::Http,
            path: Some("/health".into()),
        },
        public_env: vec!["VITE_WS_URL".into(), "VITE_API_URL".into()],
        evidence: vec![],
    };
    let mut payload = json!({"env":{"PRIVATE_SECRET":"nope"}});
    configure_service_payload(
        &mut payload,
        &service,
        "https://patchwork.example",
        "wss://patchwork.example",
        &checkout,
    )
    .await
    .unwrap();
    assert_eq!(payload["env"]["VITE_WS_URL"], "wss://patchwork.example");
    assert_eq!(payload["env"]["VITE_API_URL"], "https://patchwork.example");
    assert!(payload["env"].get("PRIVATE_SECRET").is_none());
    assert_eq!(
        payload["_hostlet_build_env"]["VITE_WS_URL"],
        "wss://patchwork.example"
    );
    tokio::fs::remove_dir_all(checkout).await.unwrap();
}

#[tokio::test]
async fn pnpm_repair_changes_only_compatible_importer_metadata() {
    let checkout = temp_checkout("repair");
    tokio::fs::create_dir_all(&checkout).await.unwrap();
    let manifest = r#"{"devDependencies":{"@playwright/test":"^1.61.1"}}"#;
    let lock = r#"lockfileVersion: '9.0'
importers:
  packages/client:
    devDependencies:
      '@playwright/test':
        specifier: ^1.53.2
        version: 1.61.1
packages:
  '@playwright/test@1.61.1': {}
snapshots:
  '@playwright/test@1.61.1': {}
"#;
    tokio::fs::write(checkout.join("pnpm-lock.yaml"), lock)
        .await
        .unwrap();
    let receipt = repair_pnpm_lock_metadata(&checkout, &patchwork_inventory(manifest))
        .await
        .unwrap()
        .unwrap();
    let repaired = tokio::fs::read_to_string(checkout.join("pnpm-lock.yaml"))
        .await
        .unwrap();
    assert!(repaired.contains("specifier: ^1.61.1"));
    assert!(repaired.contains("'@playwright/test@1.61.1'"));
    assert_eq!(receipt["changedSpecifiers"], 1);
    assert_eq!(receipt["resolvedGraphUnchanged"], true);
    assert_eq!(receipt["ephemeral"], true);
    assert_ne!(receipt["beforeSha256"], receipt["afterSha256"]);
    tokio::fs::remove_dir_all(checkout).await.unwrap();
}

#[tokio::test]
async fn pnpm_repair_fails_closed_for_resolution_change() {
    let checkout = temp_checkout("reject");
    tokio::fs::create_dir_all(&checkout).await.unwrap();
    let manifest = r#"{"devDependencies":{"@playwright/test":"^2.0.0"}}"#;
    let lock = r#"lockfileVersion: '9.0'
importers:
  packages/client:
    devDependencies:
      '@playwright/test':
        specifier: ^1.53.2
        version: 1.61.1
packages: {}
snapshots: {}
"#;
    let path = checkout.join("pnpm-lock.yaml");
    tokio::fs::write(&path, lock).await.unwrap();
    let err = repair_pnpm_lock_metadata(&checkout, &patchwork_inventory(manifest))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("run pnpm install"));
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), lock);
    tokio::fs::remove_dir_all(checkout).await.unwrap();
}
