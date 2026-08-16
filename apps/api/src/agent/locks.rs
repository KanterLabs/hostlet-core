//! Canonical row-lock helpers for app-bound lifecycle transactions.
//!
//! Lock app rows first, then deployments, then agent jobs, and finally the
//! assigned server when a transaction needs all four. IDs within a table are
//! acquired in ascending order. Transactions that do not need one of the rows
//! simply skip that step; they must never acquire a later row before an earlier
//! row in this order.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(crate) async fn app(tx: &mut Transaction<'_, Postgres>, app_id: Uuid) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar::<_, Uuid>("SELECT id FROM apps WHERE id=$1 FOR UPDATE")
        .bind(app_id)
        .fetch_optional(&mut **tx)
        .await?
        .is_some();
    anyhow::ensure!(exists, "app no longer exists");
    Ok(())
}

pub(crate) async fn deployment(
    tx: &mut Transaction<'_, Postgres>,
    deployment_id: Uuid,
    app_id: Uuid,
) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM deployments WHERE id=$1 AND app_id=$2 FOR UPDATE",
    )
    .bind(deployment_id)
    .bind(app_id)
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    anyhow::ensure!(exists, "deployment no longer exists");
    Ok(())
}

pub(crate) async fn job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar::<_, Uuid>(
        "SELECT id
         FROM agent_jobs
         WHERE id=$1
           AND ($2::uuid IS NULL OR app_id=$2)
           AND ($3::uuid IS NULL OR deployment_id=$3)
         FOR UPDATE",
    )
    .bind(job_id)
    .bind(app_id)
    .bind(deployment_id)
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    anyhow::ensure!(exists, "job no longer exists");
    Ok(())
}

pub(crate) async fn server(
    tx: &mut Transaction<'_, Postgres>,
    server_id: Uuid,
) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar::<_, Uuid>("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
        .bind(server_id)
        .fetch_optional(&mut **tx)
        .await?
        .is_some();
    anyhow::ensure!(exists, "server no longer exists");
    Ok(())
}

pub(crate) async fn apps(tx: &mut Transaction<'_, Postgres>, ids: &[Uuid]) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT id FROM apps WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn deployments(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[Uuid],
) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT id FROM deployments WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn jobs(tx: &mut Transaction<'_, Postgres>, ids: &[Uuid]) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT id FROM agent_jobs WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
