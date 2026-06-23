use super::*;
use std::collections::BTreeMap;
use uuid::Uuid;

const STORAGE_DF_TIMEOUT: Duration = Duration::from_secs(30);
/// Single-service apps create their data volume directly as
/// `hostlet-app-data-<uuid>` (see `app_data_volume`).
const SINGLE_VOLUME_PREFIX: &str = "hostlet-app-data-";
/// Compose apps namespace their volumes under the project
/// `hostlet-app-<simple-uuid>` (see `compose_project_name`).
const COMPOSE_PROJECT_PREFIX: &str = "hostlet-app-";
/// Built deployment images are tagged `hostlet/app-<app-uuid>:<deployment-uuid>`
/// (see `runtime::pipeline`). The repository segment alone identifies the owning
/// app, and tenant web containers run that exact image, so this one prefix maps
/// both image disk usage and the running container's writable layer to an app.
const IMAGE_REPO_PREFIX: &str = "hostlet/app-";

/// One app's disk footprint, sampled per cycle: the managed volume(s) (gated by
/// the storage quota), the built image, and the running container writable
/// layer. Only the volume total is authoritative for the quota; image and
/// container bytes are display-only.
#[derive(Default)]
struct AppFootprint {
    volumes: Vec<(String, i64)>,
    image_bytes: i64,
    container_bytes: i64,
}

/// Measures each Hostlet-managed app's full disk footprint and reports it
/// per app: the managed volume total (plus a per-volume breakdown), the built
/// deployment image size, and the running container's writable layer.
///
/// The managed-volume measurement is the gating prerequisite: if `docker system
/// df -v` fails we report nothing (preserving the over-quota gate's last good
/// sample). Image and container sampling are best-effort additions layered onto
/// the apps that already have a volume this cycle, so a transient failure of
/// either only blanks the display fields for one 60 s tick, never the quota.
pub(crate) async fn publish_storage_stats(cfg: &Config) {
    let Some(volumes) = docker_df_section("{{json .Volumes}}").await else {
        return;
    };
    let mut by_app: BTreeMap<Uuid, AppFootprint> = BTreeMap::new();
    for volume in &volumes {
        let name = volume.get("Name").and_then(|v| v.as_str()).unwrap_or("");
        let labels = volume.get("Labels").and_then(|v| v.as_str()).unwrap_or("");
        let Some((app_id, logical)) = volume_app_and_name(name, labels) else {
            continue;
        };
        let Some(bytes) = volume
            .get("Size")
            .and_then(|v| v.as_str())
            .and_then(parse_docker_bytes)
        else {
            continue;
        };
        by_app
            .entry(app_id)
            .or_default()
            .volumes
            .push((logical, bytes));
    }
    add_image_usage(&mut by_app).await;
    add_container_writable_usage(&mut by_app).await;
    for (app_id, footprint) in by_app {
        let volume_total: i64 = footprint.volumes.iter().map(|(_, bytes)| bytes).sum();
        let volumes_json: Vec<Value> = footprint
            .volumes
            .into_iter()
            .map(|(name, bytes)| json!({ "name": name, "usedBytes": bytes }))
            .collect();
        post(
            cfg,
            json!({
                "type": "storage_stats",
                "appId": app_id,
                "usedBytes": volume_total,
                "imageBytes": footprint.image_bytes,
                "containerBytes": footprint.container_bytes,
                "volumes": volumes_json,
            }),
        )
        .await;
    }
}

/// Runs `docker system df -v` with the given `--format` template and parses the
/// JSON array it prints, or `None` if the command fails or the output is not a
/// JSON array. One verbose `system df` call is the only portable source of
/// computed image/volume sizes (`docker images`/`volume inspect` report `N/A`).
async fn docker_df_section(format: &str) -> Option<Vec<Value>> {
    let output = command_output(
        "docker",
        &["system", "df", "-v", "--format", format],
        STORAGE_DF_TIMEOUT,
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<Vec<Value>>(stdout.trim()).ok()
}

/// Adds each app's built-image disk usage onto its existing footprint entry.
/// Sums every retained `hostlet/app-<uuid>:*` image (old deployment images keep
/// occupying disk until compose cleanup runs). Only apps already present from
/// the volume pass are updated, so an image never resurrects a volume-less entry.
async fn add_image_usage(by_app: &mut BTreeMap<Uuid, AppFootprint>) {
    let Some(images) = docker_df_section("{{json .Images}}").await else {
        return;
    };
    for image in &images {
        let Some(app_id) = image
            .get("Repository")
            .and_then(|v| v.as_str())
            .and_then(image_ref_app)
        else {
            continue;
        };
        let Some(bytes) = image
            .get("Size")
            .and_then(|v| v.as_str())
            .and_then(parse_docker_bytes)
        else {
            continue;
        };
        if let Some(footprint) = by_app.get_mut(&app_id) {
            footprint.image_bytes = footprint.image_bytes.saturating_add(bytes);
        }
    }
}

/// Adds each running app container's writable-layer size onto its footprint.
/// `docker ps -s` reports size as "<writable> (virtual <total>)"; the writable
/// part is the ephemeral container layer. Containers are matched by the
/// `hostlet/app-<uuid>` image they run.
async fn add_container_writable_usage(by_app: &mut BTreeMap<Uuid, AppFootprint>) {
    let Ok(output) = command_output(
        "docker",
        &["ps", "-s", "--format", "{{json .}}"],
        STORAGE_DF_TIMEOUT,
    )
    .await
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(app_id) = row
            .get("Image")
            .and_then(|v| v.as_str())
            .and_then(image_ref_app)
        else {
            continue;
        };
        let Some(bytes) = row
            .get("Size")
            .and_then(|v| v.as_str())
            .and_then(writable_layer_bytes)
        else {
            continue;
        };
        if let Some(footprint) = by_app.get_mut(&app_id) {
            footprint.container_bytes = footprint.container_bytes.saturating_add(bytes);
        }
    }
}

/// Resolves a `hostlet/app-<uuid>` image reference (with or without a `:tag`) to
/// the owning app id. Returns `None` for any image Hostlet does not build.
fn image_ref_app(image_ref: &str) -> Option<Uuid> {
    let rest = image_ref.strip_prefix(IMAGE_REPO_PREFIX)?;
    let uuid = rest.split(':').next().unwrap_or(rest);
    Uuid::parse_str(uuid).ok()
}

/// Extracts the writable-layer byte count from `docker ps -s`'s size string,
/// e.g. "1.04MB (virtual 927MB)" -> the bytes of "1.04MB".
fn writable_layer_bytes(size: &str) -> Option<i64> {
    let writable = size.split(" (virtual").next().unwrap_or(size).trim();
    parse_docker_bytes(writable)
}

/// Resolves a Docker volume to the app it belongs to and its logical name.
/// Single-service apps create `hostlet-app-data-<uuid>` directly (no compose
/// labels); compose apps (add-ons + remapped binds) namespace volumes under the
/// `com.docker.compose.project=hostlet-app-<simple-uuid>` label. Returns `None`
/// for volumes Hostlet does not manage.
fn volume_app_and_name(name: &str, labels: &str) -> Option<(Uuid, String)> {
    if let Some(uuid) = name.strip_prefix(SINGLE_VOLUME_PREFIX) {
        let app_id = Uuid::parse_str(uuid).ok()?;
        return Some((app_id, "data".to_string()));
    }
    let project = label_value(labels, "com.docker.compose.project")?;
    let simple = project.strip_prefix(COMPOSE_PROJECT_PREFIX)?;
    let app_id = Uuid::parse_str(simple).ok()?;
    let logical = label_value(labels, "com.docker.compose.volume")
        .map(str::to_string)
        .or_else(|| {
            name.strip_prefix(&format!("{project}_"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| name.to_string());
    Some((app_id, logical))
}

/// Looks up a label value in Docker's comma-joined `k=v,k=v` label string.
fn label_value<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    labels.split(',').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_service_data_volume_maps_to_its_app() {
        let app = Uuid::from_u128(0x1234);
        let name = format!("hostlet-app-data-{app}");
        assert_eq!(
            volume_app_and_name(&name, "com.docker.volume.anonymous="),
            Some((app, "data".to_string()))
        );
    }

    #[test]
    fn compose_addon_volume_maps_via_project_label() {
        let app = Uuid::from_u128(0xabcd);
        let project = format!("hostlet-app-{}", app.simple());
        let name = format!("{project}_pgdata");
        let labels =
            format!("com.docker.compose.project={project},com.docker.compose.volume=pgdata");
        assert_eq!(
            volume_app_and_name(&name, &labels),
            Some((app, "pgdata".to_string()))
        );
    }

    #[test]
    fn remapped_bind_volume_falls_back_to_stripped_name_without_volume_label() {
        let app = Uuid::from_u128(0x55);
        let project = format!("hostlet-app-{}", app.simple());
        let name = format!("{project}_hostlet-app-data");
        let labels = format!("com.docker.compose.project={project}");
        assert_eq!(
            volume_app_and_name(&name, &labels),
            Some((app, "hostlet-app-data".to_string()))
        );
    }

    #[test]
    fn unmanaged_and_anonymous_volumes_are_ignored() {
        assert_eq!(
            volume_app_and_name("40df8530395454a22083d", "com.docker.volume.anonymous="),
            None
        );
        assert_eq!(volume_app_and_name("hostlet-app-data-not-a-uuid", ""), None);
        // A non-Hostlet compose project is not ours.
        assert_eq!(
            volume_app_and_name("infra_pgdata", "com.docker.compose.project=infra"),
            None
        );
    }

    #[test]
    fn image_ref_app_maps_repository_and_tagged_reference() {
        let app = Uuid::from_u128(0x1234);
        // Images section reports the bare repository (no tag).
        assert_eq!(image_ref_app(&format!("hostlet/app-{app}")), Some(app));
        // `docker ps -s` reports the full `repo:tag` reference.
        let deployment = Uuid::from_u128(0xabcd);
        assert_eq!(
            image_ref_app(&format!("hostlet/app-{app}:{deployment}")),
            Some(app)
        );
    }

    #[test]
    fn image_ref_app_ignores_foreign_images() {
        assert_eq!(image_ref_app("postgres:16"), None);
        assert_eq!(image_ref_app("hostlet/builder-base:latest"), None);
        assert_eq!(image_ref_app("hostlet/app-not-a-uuid:tag"), None);
    }

    #[test]
    fn writable_layer_bytes_takes_the_pre_virtual_part() {
        assert_eq!(
            writable_layer_bytes("1.04MB (virtual 927MB)"),
            Some(1_040_000)
        );
        assert_eq!(writable_layer_bytes("0B (virtual 521MB)"), Some(0));
        // No virtual suffix (rare): parse the whole value.
        assert_eq!(writable_layer_bytes("2kB"), Some(2_000));
    }
}
