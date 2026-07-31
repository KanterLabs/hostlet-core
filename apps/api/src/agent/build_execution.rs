use crate::state::AppState;
use anyhow::Context;
use serde_json::{json, Value};
use sqlx::{Postgres, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

fn required_registry_env(key: &str) -> anyhow::Result<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{key} is required to release a remote build"))
}

fn validate_digest_ref(digest_ref: &str, prefix: &str) -> anyhow::Result<()> {
    let digest = digest_ref
        .strip_prefix(prefix)
        .context("artifact digest is outside the assigned registry repository")?;
    anyhow::ensure!(
        digest.len() == 64 && digest.chars().all(|value| value.is_ascii_hexdigit()),
        "artifact manifest contains an invalid sha256 digest"
    );
    Ok(())
}

pub(crate) fn validate_build_manifest(payload: &Value, manifest: &Value) -> anyhow::Result<()> {
    if manifest.get("schemaVersion").and_then(Value::as_u64) == Some(1) {
        return validate_legacy_build_manifest(payload, manifest);
    }
    let manifest: hostlet_contracts::BuildArtifactManifestV2 =
        serde_json::from_value(manifest.clone())
            .context("build result has an invalid artifact manifest")?;
    anyhow::ensure!(
        manifest.schema_version == hostlet_contracts::BUILD_ARTIFACT_SCHEMA_VERSION,
        "build result uses an unsupported artifact manifest schema"
    );
    let expected_build = payload
        .get("build_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("build assignment is missing build_id")?;
    let expected_deployment = payload
        .get("deployment_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("build assignment is missing deployment_id")?;
    let expected_app = payload
        .get("app_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("build assignment is missing app_id")?;
    anyhow::ensure!(
        manifest.build_id == expected_build
            && manifest.deployment_id == expected_deployment
            && manifest.app_id == expected_app
            && payload.get("commit_sha").and_then(Value::as_str)
                == Some(manifest.commit_sha.as_str())
            && payload.get("required_platform").and_then(Value::as_str)
                == Some(manifest.platform.as_str()),
        "artifact manifest does not match the build assignment"
    );
    validate_sha256(&manifest.build_plan_digest, "build plan digest")?;
    let registry = payload
        .get("artifact_registry")
        .context("build assignment is missing artifact registry configuration")?;
    let host = registry
        .get("host")
        .and_then(Value::as_str)
        .context("registry host")?;
    let repository = registry
        .get("repository")
        .and_then(Value::as_str)
        .context("registry repository")?;
    let prefix = format!("{host}/{repository}@sha256:");
    validate_digest_ref(&manifest.index.digest_ref, &prefix)?;
    validate_digest_ref(&manifest.bundle.digest_ref, &prefix)?;
    anyhow::ensure!(
        manifest.index.media_type == "application/vnd.oci.image.index.v1+json",
        "artifact index has an unexpected media type"
    );
    anyhow::ensure!(
        valid_image_manifest_media_type(&manifest.bundle.media_type),
        "release bundle has an unexpected media type"
    );
    anyhow::ensure!(
        !manifest.images.is_empty(),
        "artifact manifest must contain images"
    );
    anyhow::ensure!(
        manifest.images.len() <= 64,
        "artifact manifest contains too many images"
    );
    let mut services = HashSet::new();
    for image in &manifest.images {
        anyhow::ensure!(
            valid_artifact_service_name(&image.service),
            "artifact manifest contains an invalid service name"
        );
        anyhow::ensure!(
            services.insert(image.service.as_str()),
            "artifact services must be unique"
        );
        anyhow::ensure!(
            matches!(
                image.role.as_str(),
                "web" | "backing" | "frontend" | "backend"
            ),
            "artifact manifest contains an invalid service role"
        );
        validate_digest_ref(&image.artifact.digest_ref, &prefix)?;
        anyhow::ensure!(
            valid_image_manifest_media_type(&image.artifact.media_type),
            "artifact image has an unexpected media type"
        );
    }
    anyhow::ensure!(
        manifest
            .images
            .iter()
            .any(|image| matches!(image.role.as_str(), "web" | "frontend")),
        "artifact manifest must identify a routed service"
    );
    Ok(())
}

fn valid_image_manifest_media_type(value: &str) -> bool {
    matches!(
        value,
        "application/vnd.oci.image.manifest.v1+json"
            | "application/vnd.docker.distribution.manifest.v2+json"
    )
}

fn valid_artifact_service_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains("..")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '@' | '/')
        })
}

fn validate_legacy_build_manifest(payload: &Value, manifest: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        manifest.get("transport").and_then(Value::as_str) == Some("oci"),
        "legacy build result must use OCI transport"
    );
    for (manifest_field, payload_field) in [
        ("buildId", "build_id"),
        ("commitSha", "commit_sha"),
        ("platform", "required_platform"),
    ] {
        anyhow::ensure!(
            manifest.get(manifest_field) == payload.get(payload_field),
            "legacy artifact manifest does not match the build assignment"
        );
    }
    let registry = payload
        .get("artifact_registry")
        .context("build assignment is missing artifact registry configuration")?;
    let prefix = format!(
        "{}/{}@sha256:",
        registry
            .get("host")
            .and_then(Value::as_str)
            .context("registry host")?,
        registry
            .get("repository")
            .and_then(Value::as_str)
            .context("registry repository")?
    );
    let images = manifest
        .get("images")
        .and_then(Value::as_array)
        .filter(|images| !images.is_empty())
        .context("legacy artifact manifest must contain an image")?;
    for image in images {
        validate_digest_ref(
            image
                .get("digestRef")
                .and_then(Value::as_str)
                .context("legacy artifact image is missing digestRef")?,
            &prefix,
        )?;
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> anyhow::Result<()> {
    let digest = value
        .strip_prefix("sha256:")
        .with_context(|| format!("{field} is not sha256-qualified"))?;
    anyhow::ensure!(
        digest.len() == 64 && digest.chars().all(|value| value.is_ascii_hexdigit()),
        "{field} is invalid"
    );
    Ok(())
}

pub(crate) async fn complete_successful_build(
    _state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    deployment_id: Uuid,
    app_id: Uuid,
    original_payload: &Value,
    manifest: &Value,
) -> anyhow::Result<()> {
    validate_build_manifest(original_payload, manifest)?;
    let runner_server_id: Uuid =
        sqlx::query_scalar("SELECT server_id FROM deployments WHERE id=$1 FOR UPDATE")
            .bind(deployment_id)
            .fetch_one(&mut **tx)
            .await?;

    let mut release_payload = original_payload.clone();
    let object = release_payload
        .as_object_mut()
        .context("build job payload is not an object")?;
    object.insert("type".into(), json!("release"));
    object.insert("artifact_manifest".into(), manifest.clone());
    object.remove("github_token");
    let registry = object
        .get_mut("artifact_registry")
        .and_then(Value::as_object_mut)
        .context("build job registry configuration is invalid")?;
    registry.remove("pushUsername");
    registry.remove("pushPassword");
    registry.insert(
        "pullUsername".into(),
        json!(required_registry_env(
            "HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME"
        )?),
    );
    registry.insert(
        "pullPassword".into(),
        json!(required_registry_env(
            "HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD"
        )?),
    );

    let build_plan_digest = manifest.get("buildPlanDigest").and_then(Value::as_str);
    let updated = sqlx::query(
        "UPDATE deployment_builds
         SET status='succeeded',artifact_manifest_json=$1,build_plan_digest=$2,
             finished_at=now(),updated_at=now()
         WHERE deployment_id=$3 AND status IN ('leased','building','publishing')",
    )
    .bind(manifest)
    .bind(build_plan_digest)
    .bind(deployment_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    anyhow::ensure!(updated == 1, "deployment build is no longer active");
    sqlx::query(
        "UPDATE deployments
         SET status='queued_for_release',artifact_manifest_json=$1,last_heartbeat_at=now()
         WHERE id=$2",
    )
    .bind(manifest)
    .bind(deployment_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,priority,protocol_version)
         VALUES ($1,$2,$3,'release','queued',$4,10,6)",
    )
    .bind(runner_server_id)
    .bind(app_id)
    .bind(deployment_id)
    .bind(release_payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn fail_build(
    tx: &mut Transaction<'_, Postgres>,
    deployment_id: Uuid,
    status: &str,
    failure: Option<&str>,
) -> anyhow::Result<()> {
    let build_status = if status == "cancelled" {
        "canceled"
    } else {
        "failed"
    };
    sqlx::query(
        "UPDATE deployment_builds
         SET status=$1,failure_summary=$2,finished_at=now(),updated_at=now()
         WHERE deployment_id=$3 AND status NOT IN ('succeeded','failed','canceled')",
    )
    .bind(build_status)
    .bind(failure)
    .bind(deployment_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_service_names_support_scoped_workspace_packages() {
        assert!(valid_artifact_service_name("@hostlet-topology/client"));
        assert!(valid_artifact_service_name("compose_web-1"));
        assert!(!valid_artifact_service_name("../other-app"));
        assert!(!valid_artifact_service_name("scope//service"));
    }

    #[test]
    fn manifest_must_match_the_assigned_repository() {
        let payload = json!({
            "build_id": "410c8450-85ba-4f64-a2f7-46807c4bc880",
            "deployment_id": "510c8450-85ba-4f64-a2f7-46807c4bc880",
            "app_id": "610c8450-85ba-4f64-a2f7-46807c4bc880",
            "commit_sha": "0123456789012345678901234567890123456789",
            "required_platform": "linux/amd64",
            "artifact_registry": {"host":"registry.test","repository":"hostlet/apps/a/artifacts"}
        });
        let manifest = json!({
            "schemaVersion": 2,
            "buildId": "410c8450-85ba-4f64-a2f7-46807c4bc880",
            "deploymentId": "510c8450-85ba-4f64-a2f7-46807c4bc880",
            "appId": "610c8450-85ba-4f64-a2f7-46807c4bc880",
            "commitSha": "0123456789012345678901234567890123456789",
            "platform": "linux/amd64",
            "buildPlanDigest": format!("sha256:{}", "b".repeat(64)),
            "index": {"digestRef": format!("registry.test/hostlet/apps/a/artifacts@sha256:{}", "c".repeat(64)), "mediaType":"application/vnd.oci.image.index.v1+json", "sizeBytes": 10},
            "bundle": {"digestRef": format!("registry.test/hostlet/apps/a/artifacts@sha256:{}", "d".repeat(64)), "mediaType":"application/vnd.oci.image.manifest.v1+json", "sizeBytes": 10},
            "images": [{
                "service":"web","role":"web","source":"built",
                "artifact": {"digestRef": format!("registry.test/other/web@sha256:{}", "a".repeat(64)), "mediaType":"application/vnd.oci.image.manifest.v1+json", "sizeBytes":10}
            }]
        });
        assert!(validate_build_manifest(&payload, &manifest).is_err());
    }
}
