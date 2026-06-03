use super::inspection::InspectionBase;
use serde_json::{json, Value};

pub(super) struct DockerfileInference {
    pub(super) port: Option<i32>,
    pub(super) env: Vec<Value>,
    pub(super) warnings: Vec<String>,
}

pub(super) struct PackageInference {
    pub(super) framework: &'static str,
    pub(super) package_manager: &'static str,
}

pub(super) fn infer_package_json(
    contents: &str,
    has_bun_lock: bool,
    has_pnpm_lock: bool,
    has_yarn_lock: bool,
) -> PackageInference {
    let package: Value = serde_json::from_str(contents).unwrap_or_else(|_| json!({}));
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
    let fallback_package_manager = if has_bun_lock {
        "bun"
    } else if has_pnpm_lock {
        "pnpm"
    } else if has_yarn_lock {
        "yarn"
    } else {
        "npm"
    };
    let package_manager = package
        .get("packageManager")
        .and_then(|value| value.as_str())
        .and_then(package_manager_from_field)
        .unwrap_or(fallback_package_manager);
    PackageInference {
        framework,
        package_manager,
    }
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

pub(super) fn infer_dockerfile(contents: &str) -> DockerfileInference {
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
                    env.push(
                        json!({"key": key, "required": false, "value": "", "source": "Dockerfile ENV"}),
                    );
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

/// The generated Hostlet Compose manifest Hostlet proposes for a Gitea deploy.
/// Kept as a readable raw literal (rather than an escaped one-line JSON string)
/// so the YAML can be reviewed and edited directly. The trailing newline is
/// significant and matches the previous escaped form byte-for-byte.
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

pub(super) fn gitea_inspection(repo: &str, branch: &str, default_branch: &str) -> Value {
    json!({
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

pub(super) fn railpack_inspection(
    repo: &str,
    branch: &str,
    default_branch: &str,
    language: &str,
) -> serde_json::Map<String, Value> {
    InspectionBase {
        repo,
        default_branch,
        branch,
        deployable: true,
        container_port: json!(3000),
        packaging_options: json!(["auto", "generated"]),
        recommended_packaging_strategy: "generated",
        env: json!([]),
        warnings: json!([format!("{language} app detected. Hostlet will build it with Railpack if there is no repository Dockerfile.")]),
        summary: format!("{language} app detected. Hostlet will use generated Railpack runtime support."),
    }
    .build()
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

#[cfg(test)]
mod tests {
    use super::{gitea_inspection, infer_dockerfile, infer_package_json, railpack_inspection};
    use serde_json::Value;

    #[test]
    fn dockerfile_inference_prefers_web_port_and_prompts_env() {
        let inference = infer_dockerfile(
            r#"
FROM alpine
ENV APP_SECRET=
ARG BUILD_TOKEN
EXPOSE 22 3000/tcp
VOLUME ["/data"]
"#,
        );
        assert_eq!(inference.port, Some(3000));
        assert!(inference.env.iter().any(|item| item["key"] == "APP_SECRET"));
        assert!(inference
            .warnings
            .iter()
            .any(|warning| warning.contains("multiple ports")));
        assert!(inference
            .warnings
            .iter()
            .any(|warning| warning.contains("BUILD_TOKEN")));
    }

    #[test]
    fn gitea_inspection_returns_generated_compose() {
        let value = gitea_inspection("go-gitea/gitea", "main", "main");
        assert_eq!(value["deployable"], true);
        assert_eq!(value["runtimeKind"], "compose");
        assert_eq!(
            value.pointer("/runtimeConfig/generatedCompose/webService"),
            Some(&serde_json::json!("server"))
        );
        assert!(value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("SSH Git access")));
    }

    #[test]
    fn railpack_inspection_marks_supported_language_deployable() {
        let value = Value::Object(railpack_inspection("owner/repo", "main", "main", "Python"));
        assert_eq!(value["deployable"], true);
        assert_eq!(value["recommendedPackagingStrategy"], "generated");
        assert!(value["summary"]
            .as_str()
            .unwrap()
            .contains("generated Railpack runtime support"));
    }

    #[test]
    fn package_json_inference_detects_framework_and_package_manager() {
        let inference = infer_package_json(
            r#"{"dependencies":{"next":"16.0.0"},"devDependencies":{}}"#,
            false,
            true,
            false,
        );
        assert_eq!(inference.framework, "Next.js");
        assert_eq!(inference.package_manager, "pnpm");
    }

    #[test]
    fn package_json_inference_detects_bun_package_manager() {
        let inference = infer_package_json(
            r#"{"scripts":{"start":"bun server.js"},"packageManager":"bun@1.3.5"}"#,
            false,
            false,
            false,
        );
        assert_eq!(inference.framework, "Node");
        assert_eq!(inference.package_manager, "bun");

        let lock_inference = infer_package_json(r#"{"dependencies":{}}"#, true, false, false);
        assert_eq!(lock_inference.package_manager, "bun");
    }

    #[test]
    fn package_json_inference_detects_yarn_package_manager() {
        let inference = infer_package_json(
            r#"{"scripts":{"start":"node server.js"},"packageManager":"yarn@1.22.22"}"#,
            false,
            false,
            false,
        );
        assert_eq!(inference.package_manager, "yarn");

        let lock_inference = infer_package_json(r#"{"dependencies":{}}"#, false, false, true);
        assert_eq!(lock_inference.package_manager, "yarn");
    }

    #[test]
    fn package_json_inference_ignores_unsupported_package_manager_field() {
        let inference = infer_package_json(
            r#"{"scripts":{"start":"node server.js"},"packageManager":"deno@2.0.0"}"#,
            false,
            true,
            false,
        );

        assert_eq!(inference.package_manager, "pnpm");
    }

    #[test]
    fn package_json_inference_prefers_supported_package_manager_field_over_locks() {
        let inference = infer_package_json(
            r#"{"scripts":{"start":"node server.js"},"packageManager":"npm@11.0.0"}"#,
            true,
            true,
            true,
        );

        assert_eq!(inference.package_manager, "npm");
    }

    #[test]
    fn package_json_inference_falls_back_when_package_json_is_malformed() {
        let locked = infer_package_json("{not-json", false, true, false);
        let unlocked = infer_package_json("{not-json", false, false, false);

        assert_eq!(locked.framework, "Node");
        assert_eq!(locked.package_manager, "pnpm");
        assert_eq!(unlocked.package_manager, "npm");
    }
}
