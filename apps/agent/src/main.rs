use anyhow::{bail, Context};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

mod build;
mod compose;
mod ops;
mod runtime;
mod validation;

pub(crate) use build::*;
pub(crate) use compose::*;
pub(crate) use ops::*;
pub(crate) use runtime::{Config, LocalRouter};
pub(crate) use validation::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    runtime::run().await
}
