//! Release-metadata model: fetching the latest GitHub release, parsing the
//! optional `hostlet-release.json` manifest, and the download/checksum helpers
//! used to validate update assets.

use super::*;
pub(crate) use hostlet_contracts::version_is_newer;

pub(crate) struct ReleaseInfo {
    pub(crate) version: String,
    pub(crate) notes_url: String,
    pub(crate) released_at: Option<String>,
    pub(crate) minimum_supported_version: Option<String>,
    pub(crate) compose_migrations: bool,
    pub(crate) database_migrations: bool,
    pub(crate) assets: Vec<ReleaseAsset>,
    pub(crate) image_registry: Option<String>,
    pub(crate) image_tag: Option<String>,
    pub(crate) images: ReleaseImages,
}

pub(crate) struct ReleaseAsset {
    name: String,
    pub(crate) download_url: String,
}

#[derive(Default)]
pub(crate) struct ReleaseImages {
    pub(crate) api: Option<ReleaseImage>,
    pub(crate) web: Option<ReleaseImage>,
    pub(crate) agent: Option<ReleaseImage>,
    pub(crate) screenshotter: Option<ReleaseImage>,
}

pub(crate) struct ReleaseImage {
    pub(crate) reference: String,
    pub(crate) digest: Option<String>,
}

impl ReleaseInfo {
    pub(crate) fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }

    pub(crate) fn image_tag(&self) -> String {
        self.image_tag
            .clone()
            .unwrap_or_else(|| format!("v{}", self.version.trim_start_matches('v')))
    }

    pub(crate) fn has_release_images(&self) -> bool {
        self.images.api.is_some()
            && self.images.web.is_some()
            && self.images.agent.is_some()
            && self.images.screenshotter.is_some()
    }

    pub(crate) fn has_release_image_digests(&self) -> bool {
        self.images.all().iter().all(|image| {
            image
                .and_then(|image| image.digest.as_deref())
                .is_some_and(valid_image_digest)
        })
    }

    pub(crate) fn image_env(&self) -> anyhow::Result<BTreeMap<String, String>> {
        self.images.env()
    }
}

impl ReleaseImages {
    fn all(&self) -> [Option<&ReleaseImage>; 4] {
        [
            self.api.as_ref(),
            self.web.as_ref(),
            self.agent.as_ref(),
            self.screenshotter.as_ref(),
        ]
    }

    fn env(&self) -> anyhow::Result<BTreeMap<String, String>> {
        let mut env = BTreeMap::new();
        env.insert(
            "HOSTLET_API_IMAGE".into(),
            self.api
                .as_ref()
                .context("release is missing api image metadata")?
                .immutable_reference()?,
        );
        env.insert(
            "HOSTLET_WEB_IMAGE".into(),
            self.web
                .as_ref()
                .context("release is missing web image metadata")?
                .immutable_reference()?,
        );
        env.insert(
            "HOSTLET_AGENT_IMAGE".into(),
            self.agent
                .as_ref()
                .context("release is missing agent image metadata")?
                .immutable_reference()?,
        );
        env.insert(
            "HOSTLET_SCREENSHOTTER_IMAGE".into(),
            self.screenshotter
                .as_ref()
                .context("release is missing screenshotter image metadata")?
                .immutable_reference()?,
        );
        Ok(env)
    }
}

impl ReleaseImage {
    fn immutable_reference(&self) -> anyhow::Result<String> {
        let digest = self
            .digest
            .as_deref()
            .filter(|digest| valid_image_digest(digest))
            .with_context(|| {
                format!("release image {} is missing a valid digest", self.reference)
            })?;
        Ok(format!("{}@{}", image_repository(&self.reference), digest))
    }
}

fn valid_image_digest(digest: &str) -> bool {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn image_repository(reference: &str) -> &str {
    let without_digest = reference
        .split_once('@')
        .map_or(reference, |(repo, _)| repo);
    let last_slash = without_digest.rfind('/').unwrap_or(0);
    match without_digest.rfind(':') {
        Some(colon) if colon > last_slash => &without_digest[..colon],
        _ => without_digest,
    }
}

pub(crate) async fn latest_release(client: &reqwest::Client) -> anyhow::Result<ReleaseInfo> {
    let value: Value = client
        .get(format!(
            "https://api.github.com/repos/{HOSTLET_REPO}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let version = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("release did not include tag_name")?
        .to_string();
    let notes_url = value
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/ShaneKanterman04/Hostlet/releases/latest")
        .to_string();
    let assets = value
        .get("assets")
        .and_then(|v| v.as_array())
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|asset| {
            Some(ReleaseAsset {
                name: asset.get("name")?.as_str()?.to_string(),
                download_url: asset.get("browser_download_url")?.as_str()?.to_string(),
            })
        })
        .collect();
    let mut release = ReleaseInfo {
        version,
        notes_url,
        released_at: value
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        minimum_supported_version: None,
        compose_migrations: false,
        database_migrations: false,
        assets,
        image_registry: None,
        image_tag: None,
        images: ReleaseImages::default(),
    };
    if let Some(manifest_url) = release
        .asset("hostlet-release.json")
        .map(|asset| asset.download_url.clone())
    {
        apply_release_manifest(client, &mut release, &manifest_url).await?;
    }
    Ok(release)
}

pub(crate) async fn apply_release_manifest(
    client: &reqwest::Client,
    release: &mut ReleaseInfo,
    manifest_url: &str,
) -> anyhow::Result<()> {
    let value: Value = client
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    apply_release_manifest_value(release, &value);
    Ok(())
}

pub(crate) fn apply_release_manifest_value(release: &mut ReleaseInfo, value: &Value) {
    if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
        release.version = version.trim_start_matches('v').to_string();
    }
    if let Some(released_at) = value.get("released_at").and_then(|v| v.as_str()) {
        release.released_at = Some(released_at.to_string());
    }
    release.minimum_supported_version = value
        .get("minimum_supported_version")
        .and_then(|v| v.as_str())
        .map(|value| value.trim_start_matches('v').to_string());
    release.compose_migrations = value
        .get("compose_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(release.compose_migrations);
    release.database_migrations = value
        .get("database_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(release.database_migrations);
    if let Some(notes_url) = value.get("notes_url").and_then(|v| v.as_str()) {
        release.notes_url = notes_url.to_string();
    }
    release.image_registry = value
        .get("image_registry")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| release.image_registry.clone());
    release.image_tag = value
        .get("image_tag")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| release.image_tag.clone());
    if let Some(images) = value.get("images").and_then(|v| v.as_object()) {
        release.images.api = parse_release_image(images.get("api")).or(release.images.api.take());
        release.images.web = parse_release_image(images.get("web")).or(release.images.web.take());
        release.images.agent =
            parse_release_image(images.get("agent")).or(release.images.agent.take());
        release.images.screenshotter = parse_release_image(images.get("screenshotter"))
            .or(release.images.screenshotter.take());
    }
}

pub(crate) fn parse_release_image(value: Option<&Value>) -> Option<ReleaseImage> {
    let value = value?;
    Some(ReleaseImage {
        reference: value.get("ref")?.as_str()?.to_string(),
        digest: value
            .get("digest")
            .and_then(|v| v.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

pub(crate) fn print_update_check(release: &ReleaseInfo) {
    let current = env!("CARGO_PKG_VERSION");
    let latest = release.version.trim_start_matches('v');
    println!("Current version: {current}");
    println!("Latest version:  {latest}");
    if let Some(minimum) = &release.minimum_supported_version {
        println!("Minimum version: {minimum}");
    }
    if release.compose_migrations || release.database_migrations {
        println!(
            "Migrations:      compose={} database={}",
            release.compose_migrations, release.database_migrations
        );
    }
    if release.has_release_images() {
        println!("Image tag:       {}", release.image_tag());
        println!(
            "Images:          api={} web={} agent={} screenshotter={}",
            release
                .images
                .api
                .as_ref()
                .map_or("missing", |image| image.reference.as_str()),
            release
                .images
                .web
                .as_ref()
                .map_or("missing", |image| image.reference.as_str()),
            release
                .images
                .agent
                .as_ref()
                .map_or("missing", |image| image.reference.as_str()),
            release
                .images
                .screenshotter
                .as_ref()
                .map_or("missing", |image| image.reference.as_str())
        );
        let signed_digests = [
            release.images.api.as_ref(),
            release.images.web.as_ref(),
            release.images.agent.as_ref(),
            release.images.screenshotter.as_ref(),
        ]
        .iter()
        .filter(|image| image.and_then(|image| image.digest.as_ref()).is_some())
        .count();
        println!("Image digests:   {signed_digests}/4 available");
    }
    println!(
        "Checksum signing: {}",
        if release.asset("hostlet-linux-x64.sha256.asc").is_some() {
            "available"
        } else {
            "unsigned checksum only"
        }
    );
    println!("Release notes:   {}", release.notes_url);
    println!(
        "Update:          {}",
        if version_is_newer(current, latest) {
            "available"
        } else {
            "not available"
        }
    );
}

pub(crate) async fn download(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
) -> anyhow::Result<()> {
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    fs::write(path, &bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) async fn checksum_from_asset(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
) -> anyhow::Result<String> {
    let text = client
        .get(&asset.download_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .context("checksum asset was empty")
}

pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_release() -> ReleaseInfo {
        ReleaseInfo {
            version: "0.5.0".into(),
            notes_url: "https://example.test/notes".into(),
            released_at: None,
            minimum_supported_version: None,
            compose_migrations: false,
            database_migrations: false,
            assets: Vec::new(),
            image_registry: None,
            image_tag: None,
            images: ReleaseImages::default(),
        }
    }

    fn full_images() -> ReleaseImages {
        let image = |reference: &str| {
            Some(ReleaseImage {
                reference: reference.into(),
                digest: None,
            })
        };
        ReleaseImages {
            api: image("api"),
            web: image("web"),
            agent: image("agent"),
            screenshotter: image("screenshotter"),
        }
    }

    #[test]
    fn image_tag_falls_back_to_v_prefixed_version() {
        let mut release = base_release();
        release.version = "1.2.3".into();
        assert_eq!(release.image_tag(), "v1.2.3");
    }

    #[test]
    fn image_tag_does_not_double_prefix_existing_v() {
        let mut release = base_release();
        release.version = "v1.2.3".into();
        assert_eq!(release.image_tag(), "v1.2.3");
    }

    #[test]
    fn image_tag_prefers_explicit_tag_over_version() {
        let mut release = base_release();
        release.version = "1.2.3".into();
        release.image_tag = Some("v9.9.9-rc1".into());
        assert_eq!(release.image_tag(), "v9.9.9-rc1");
    }

    #[test]
    fn has_release_images_requires_all_four_images() {
        let mut release = base_release();
        assert!(!release.has_release_images());

        release.images = full_images();
        assert!(release.has_release_images());

        release.images.agent = None;
        assert!(!release.has_release_images());
    }

    #[test]
    fn asset_lookup_matches_by_exact_name() {
        let mut release = base_release();
        release.assets = vec![
            ReleaseAsset {
                name: "hostlet-linux-x64".into(),
                download_url: "https://example.test/bin".into(),
            },
            ReleaseAsset {
                name: "hostlet-linux-x64.sha256".into(),
                download_url: "https://example.test/sum".into(),
            },
        ];

        assert_eq!(
            release
                .asset("hostlet-linux-x64")
                .map(|a| a.download_url.as_str()),
            Some("https://example.test/bin")
        );
        assert_eq!(
            release
                .asset("hostlet-linux-x64.sha256")
                .map(|a| a.download_url.as_str()),
            Some("https://example.test/sum")
        );
        assert!(release.asset("missing").is_none());
    }

    #[test]
    fn parse_release_image_reads_ref_and_digest() {
        let value = serde_json::json!({ "ref": "ghcr.io/x:1", "digest": "sha256:abc" });
        let image = parse_release_image(Some(&value)).unwrap();
        assert_eq!(image.reference, "ghcr.io/x:1");
        assert_eq!(image.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn parse_release_image_drops_empty_digest() {
        let value = serde_json::json!({ "ref": "ghcr.io/x:1", "digest": "" });
        let image = parse_release_image(Some(&value)).unwrap();
        assert_eq!(image.reference, "ghcr.io/x:1");
        assert!(image.digest.is_none());
    }

    #[test]
    fn parse_release_image_requires_ref() {
        assert!(parse_release_image(None).is_none());
        assert!(
            parse_release_image(Some(&serde_json::json!({ "digest": "sha256:abc" }))).is_none()
        );
    }

    #[test]
    fn image_env_uses_digest_refs_and_rejects_missing_digest() {
        let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut release = base_release();
        release.images = ReleaseImages {
            api: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-api:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
            web: Some(ReleaseImage {
                reference: "localhost:5000/example/hostlet-web:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
            agent: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-agent@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                digest: Some(digest.into()),
            }),
            screenshotter: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-screenshotter:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
        };

        let env = release.image_env().unwrap();

        assert_eq!(
            env.get("HOSTLET_API_IMAGE").map(String::as_str),
            Some("ghcr.io/example/hostlet-api@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            env.get("HOSTLET_WEB_IMAGE").map(String::as_str),
            Some("localhost:5000/example/hostlet-web@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        release.images.api.as_mut().unwrap().digest = None;
        assert!(release.image_env().is_err());
    }

    #[test]
    fn apply_manifest_trims_v_prefix_from_versions() {
        let mut release = base_release();
        let manifest = serde_json::json!({
            "version": "v2.0.0",
            "minimum_supported_version": "v1.5.0",
            "compose_migrations": true,
            "database_migrations": true,
            "notes_url": "https://example.test/v2"
        });

        apply_release_manifest_value(&mut release, &manifest);

        assert_eq!(release.version, "2.0.0");
        assert_eq!(release.minimum_supported_version.as_deref(), Some("1.5.0"));
        assert!(release.compose_migrations);
        assert!(release.database_migrations);
        assert_eq!(release.notes_url, "https://example.test/v2");
    }

    #[test]
    fn apply_manifest_preserves_existing_image_tag_when_absent() {
        let mut release = base_release();
        release.image_tag = Some("v3.3.3".into());

        apply_release_manifest_value(&mut release, &serde_json::json!({}));

        assert_eq!(release.image_tag(), "v3.3.3");
    }
}
