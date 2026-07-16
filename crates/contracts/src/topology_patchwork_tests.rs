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
fn patchwork_shape_is_frontend_backend_not_workspace_libraries() {
    let plan = plan_repository_topology(&inventory(&[
        (
            "package.json",
            r#"{"name":"patchwork","scripts":{"build":"pnpm -r build"}}"#,
        ),
        ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
        ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        (
            "packages/client/package.json",
            r#"{"name":"@patchwork/client","scripts":{"build":"vite build"},"devDependencies":{"vite":"^7"}}"#,
        ),
        (
            "packages/server/package.json",
            r#"{"name":"@patchwork/server","scripts":{"build":"tsc -b","start":"tsx src/index.ts"},"dependencies":{"ws":"^8"}}"#,
        ),
        ("packages/content/package.json", build_only("content")),
        ("packages/e2e/package.json", build_only("e2e")),
        ("packages/harness/package.json", build_only("harness")),
        ("packages/protocol/package.json", build_only("protocol")),
        ("packages/sim/package.json", build_only("sim")),
        ("packages/spritegen/package.json", build_only("spritegen")),
        (
            "packages/client/src/main.ts",
            "connect(import.meta.env.VITE_WS_URL);",
        ),
    ]));
    assert_eq!(plan.readiness, TopologyReadiness::Ready);
    assert_eq!(plan.services.len(), 2);
    assert_eq!(plan.candidates.len(), 2);
    assert_eq!(plan.services[0].role, ServiceRole::Frontend);
    assert_eq!(plan.services[0].public_env, vec!["VITE_WS_URL"]);
    assert_eq!(
        plan.services[0].build_command.as_deref(),
        Some("pnpm --filter @patchwork/client... run build")
    );
    assert_eq!(plan.services[1].role, ServiceRole::Backend);
    assert_eq!(plan.services[1].health_probe.kind, HealthProbeKind::Tcp);
}

fn build_only(name: &str) -> &'static str {
    match name {
        "content" => r#"{"name":"content","scripts":{"build":"tsc"}}"#,
        "e2e" => r#"{"name":"e2e","scripts":{"build":"tsc","run":"tsx run.ts"}}"#,
        "harness" => r#"{"name":"harness","scripts":{"build":"tsc","balance":"tsx run.ts"}}"#,
        "protocol" => r#"{"name":"protocol","scripts":{"build":"tsc"}}"#,
        "sim" => r#"{"name":"sim","scripts":{"build":"tsc"}}"#,
        "spritegen" => r#"{"name":"spritegen","scripts":{"build":"tsc","spritegen":"tsx cli.ts"}}"#,
        _ => unreachable!(),
    }
}
