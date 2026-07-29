use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const HOSTLET_REPO: &str = "KanterLabs/hostlet-core";
const HOSTLET_RELEASES_LATEST_URL: &str =
    "https://github.com/KanterLabs/hostlet-core/releases/latest";

fn linux_asset_for_arch(arch: &str) -> anyhow::Result<&'static str> {
    match arch {
        "x86_64" => Ok("hostlet-linux-x64"),
        arch => bail!("Hostlet stable releases support Linux x86_64 only (got {arch})"),
    }
}

fn linux_asset() -> anyhow::Result<&'static str> {
    linux_asset_for_arch(std::env::consts::ARCH)
}

mod backups;
mod cloudflare;
mod compose;
mod doctor;
mod runtime;
mod update;
mod util;

use backups::*;
use cloudflare::*;
use compose::*;
use doctor::*;
use update::*;
use util::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    runtime::run().await
}

#[cfg(test)]
mod tests {
    use super::linux_asset_for_arch;

    #[test]
    fn release_asset_is_x86_64_only() {
        assert_eq!(
            linux_asset_for_arch("x86_64").expect("x86_64 should be supported"),
            "hostlet-linux-x64"
        );
        assert!(linux_asset_for_arch("aarch64").is_err());
        assert!(linux_asset_for_arch("arm").is_err());
    }
}
