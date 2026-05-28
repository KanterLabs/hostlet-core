#[tokio::main]
async fn main() -> anyhow::Result<()> {
    hostlet_api::run_from_env().await
}
