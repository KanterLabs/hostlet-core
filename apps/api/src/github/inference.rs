pub(super) use hostlet_contracts::{
    dockerfile_inspection, gitea_inspection, infer_dockerfile, infer_package_json, node_inspection,
    railpack_inspection, unknown_inspection,
};

#[cfg(test)]
mod tests {
    use super::{gitea_inspection, infer_dockerfile, infer_package_json, railpack_inspection};

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
        let value = railpack_inspection("owner/repo", "main", "main", "Python");
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
}
