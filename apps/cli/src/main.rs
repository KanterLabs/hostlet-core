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

const HOSTLET_REPO: &str = "ShaneKanterman04/Hostlet";
fn linux_asset() -> anyhow::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("hostlet-linux-x64"),
        "aarch64" => Ok("hostlet-linux-arm64"),
        arch => bail!("Hostlet releases do not support Linux architecture {arch}"),
    }
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
