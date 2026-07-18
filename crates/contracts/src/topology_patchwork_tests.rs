use super::*;
use std::path::Path;

#[test]
fn patchwork_shape_is_frontend_backend_not_workspace_libraries() {
    let plan = plan_repository_topology(&patchwork_fixture_inventory());
    assert_eq!(plan.readiness, TopologyReadiness::Ready);
    assert_eq!(plan.services.len(), 2);
    assert_eq!(plan.candidates.len(), 2);
    assert_eq!(plan.services[0].role, ServiceRole::Frontend);
    assert_eq!(plan.services[0].public_env, vec!["VITE_WS_URL"]);
    assert_eq!(
        plan.services[0].build_command.as_deref(),
        Some("pnpm --filter @hostlet-topology/client... run build")
    );
    assert_eq!(plan.services[1].role, ServiceRole::Backend);
    assert_eq!(plan.services[1].health_probe.kind, HealthProbeKind::Tcp);
}

fn patchwork_fixture_inventory() -> RepositoryInventory {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/fixtures/generated-apps/topology-patchwork");
    let mut files = Vec::new();
    collect_fixture_files(&root, &root, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    RepositoryInventory { files }
}

fn collect_fixture_files(root: &Path, dir: &Path, files: &mut Vec<RepositoryFile>) {
    for entry in std::fs::read_dir(dir).expect("read Patchwork fixture") {
        let entry = entry.expect("read Patchwork fixture entry");
        let path = entry.path();
        if path.is_dir() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("node_modules" | "dist" | ".git")
            ) {
                continue;
            }
            collect_fixture_files(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("fixture path is below root")
                .to_string_lossy()
                .replace('\\', "/");
            files.push(RepositoryFile {
                path: relative,
                contents: Some(std::fs::read_to_string(path).expect("read Patchwork fixture file")),
            });
        }
    }
}
