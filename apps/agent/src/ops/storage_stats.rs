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

/// Measures every Hostlet-managed Docker volume's disk usage and reports a
/// per-app total plus a per-volume breakdown. One `docker system df -v` call
/// returns computed sizes for all volumes (`docker volume ls`/`inspect` report
/// `Size: N/A`), so it is the only portable single-command source.
pub(crate) async fn publish_storage_stats(cfg: &Config) {
    let Ok(output) = command_output(
        "docker",
        &["system", "df", "-v", "--format", "{{json .Volumes}}"],
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
    let Ok(volumes) = serde_json::from_str::<Vec<Value>>(stdout.trim()) else {
        return;
    };
    let mut by_app: BTreeMap<Uuid, Vec<(String, i64)>> = BTreeMap::new();
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
        by_app.entry(app_id).or_default().push((logical, bytes));
    }
    for (app_id, vols) in by_app {
        let total: i64 = vols.iter().map(|(_, bytes)| bytes).sum();
        let volumes_json: Vec<Value> = vols
            .into_iter()
            .map(|(name, bytes)| json!({ "name": name, "usedBytes": bytes }))
            .collect();
        post(
            cfg,
            json!({
                "type": "storage_stats",
                "appId": app_id,
                "usedBytes": total,
                "volumes": volumes_json,
            }),
        )
        .await;
    }
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
}
