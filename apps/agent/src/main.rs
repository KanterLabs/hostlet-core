use anyhow::{bail, Context};
use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
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

mod build;
mod compose;
mod ops;
mod railpack;
mod runtime;
mod validation;

pub(crate) use build::*;
pub(crate) use compose::*;
pub(crate) use ops::*;
pub(crate) use railpack::*;
pub(crate) use runtime::{reported_deployment_failure, Config, LocalRouter};
pub(crate) use validation::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    runtime::run().await
}
