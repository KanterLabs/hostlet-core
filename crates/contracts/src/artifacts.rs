use crate::{GeneratedTopologyConfig, HealthProbe, InferredService};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const BUILD_ARTIFACT_SCHEMA_VERSION: u16 = 2;
pub const RELEASE_BUNDLE_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OciArtifactDescriptor {
    pub digest_ref: String,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactImageSource {
    Built,
    Mirrored,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactServiceImage {
    pub service: String,
    pub role: String,
    pub source: ArtifactImageSource,
    pub artifact: OciArtifactDescriptor,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildArtifactManifestV2 {
    pub schema_version: u16,
    pub build_id: Uuid,
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub commit_sha: String,
    pub platform: String,
    pub build_plan_digest: String,
    pub index: OciArtifactDescriptor,
    pub bundle: OciArtifactDescriptor,
    pub images: Vec<ArtifactServiceImage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerFilesystemMode {
    ReadOnly,
    Writable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEnvPolicy {
    All,
    None,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseService {
    pub name: String,
    pub role: String,
    pub image_digest_ref: String,
    pub container_port: u16,
    pub health_probe: HealthProbe,
    pub filesystem: ContainerFilesystemMode,
    pub env_policy: RuntimeEnvPolicy,
    #[serde(default)]
    pub public_env: Vec<String>,
    #[serde(default)]
    pub runtime_metadata: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReleaseRuntime {
    Single {
        service: ReleaseService,
    },
    Compose {
        compose_file: String,
        hostlet_config_path: String,
        web_service: String,
        target_port: u16,
        health_path: String,
        backing_spec_source: String,
    },
    GeneratedTopology {
        services: Vec<ReleaseService>,
        inferred_services: Vec<InferredService>,
        config: GeneratedTopologyConfig,
        inference_receipt: Value,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseBundleV1 {
    pub schema_version: u16,
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub commit_sha: String,
    pub platform: String,
    pub runtime: ReleaseRuntime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_manifest_uses_camel_case_wire_fields() {
        let value = serde_json::to_value(OciArtifactDescriptor {
            digest_ref: "registry.test/repo@sha256:abc".into(),
            media_type: "application/vnd.oci.image.manifest.v1+json".into(),
            size_bytes: 12,
        })
        .unwrap();
        assert_eq!(value["digestRef"], "registry.test/repo@sha256:abc");
        assert_eq!(value["sizeBytes"], 12);
    }
}
