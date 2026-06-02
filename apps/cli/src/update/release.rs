//! Release-metadata model: fetching the latest GitHub release, parsing the
//! optional `hostlet-release.json` manifest, and the download/checksum helpers
//! used to validate update assets.

use super::*;

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
    api: Option<ReleaseImage>,
    pub(crate) web: Option<ReleaseImage>,
    pub(crate) agent: Option<ReleaseImage>,
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
        self.images.api.is_some() && self.images.web.is_some() && self.images.agent.is_some()
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
            "Images:          api={} web={} agent={}",
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
                .map_or("missing", |image| image.reference.as_str())
        );
        let signed_digests = [
            release.images.api.as_ref(),
            release.images.web.as_ref(),
            release.images.agent.as_ref(),
        ]
        .iter()
        .filter(|image| image.and_then(|image| image.digest.as_ref()).is_some())
        .count();
        println!("Image digests:   {signed_digests}/3 available");
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

pub(crate) fn version_is_newer(current: &str, latest: &str) -> bool {
    version_parts(latest) > version_parts(current)
}

pub(crate) fn version_parts(value: &str) -> (u64, u64, u64) {
    let mut parts = value
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
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
