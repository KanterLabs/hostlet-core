use crate::{agent::authenticated_server_id, auth::request_context, deploy, state::AppState};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

mod storage;
pub(crate) use storage::sweep_orphaned_screenshot_files;

const MAX_SCREENSHOT_BYTES: usize = 1_500_000;
const GENERATED_SOURCE: &str = "generated";
const LIVE_DEPLOYMENT_STATUSES: &[&str] = &["success", "rolled_back"];
const ACTIVE_JOB_STATUSES: &[&str] = &["queued", "claimed", "running"];

#[derive(Clone, Debug)]
pub struct ScreenshotAutoCaptureCandidate {
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub server_id: Uuid,
    pub domain: String,
}

#[derive(Clone, Debug)]
pub struct StoredScreenshot {
    pub id: Uuid,
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub agent_job_id: Uuid,
    pub public_url: String,
    pub captured_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait ScreenshotHooks: Send + Sync {
    async fn allow_auto_capture(
        &self,
        _state: &AppState,
        _candidate: &ScreenshotAutoCaptureCandidate,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn after_screenshot_stored(
        &self,
        _tx: &mut Transaction<'_, Postgres>,
        _stored: &StoredScreenshot,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

pub struct NoopScreenshotHooks;

#[async_trait]
impl ScreenshotHooks for NoopScreenshotHooks {}

#[derive(Deserialize)]
pub struct ScreenshotUploadQuery {
    pub(crate) app_id: Uuid,
    pub(crate) deployment_id: Uuid,
    pub(crate) job_id: Uuid,
    pub(crate) width: Option<i32>,
    pub(crate) height: Option<i32>,
    pub(crate) capture_url: Option<String>,
}

pub async fn capture_app_screenshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> Response {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    match enqueue_screenshot_for_owner(&state, context.user_id, app_id).await {
        Ok(job_id) => (StatusCode::ACCEPTED, Json(json!({"jobId": job_id}))).into_response(),
        Err(ScreenshotQueueError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(ScreenshotQueueError::NotReady(message)) => {
            (StatusCode::BAD_REQUEST, message).into_response()
        }
        Err(ScreenshotQueueError::Internal(err)) => {
            tracing::warn!(error = %err, app_id = %app_id, "failed to queue screenshot job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn latest_app_screenshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> Response {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match latest_screenshot_for_owner(&state, context.user_id, app_id).await {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, "failed to load latest screenshot");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn public_screenshot(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let row = sqlx::query("SELECT storage_path,content_type FROM app_screenshots WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let storage_path = row.get::<String, _>("storage_path");
    if storage_path.contains('/') || storage_path.contains('\\') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let bytes = match tokio::fs::read(state.screenshot_dir.join(&storage_path)).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(err) => {
            tracing::warn!(error = %err, screenshot_id = %id, "failed to read screenshot");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let mut response = Response::new(axum::body::Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&row.get::<String, _>("content_type"))
            .unwrap_or_else(|_| HeaderValue::from_static("image/jpeg")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

pub async fn upload_agent_screenshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ScreenshotUploadQuery>,
    body: Bytes,
) -> Response {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let content_type = match screenshot_content_type(&headers) {
        Some(content_type) => content_type,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "screenshot must be image/jpeg or image/webp",
            )
                .into_response()
        }
    };
    if body.is_empty() || body.len() > MAX_SCREENSHOT_BYTES {
        return (
            StatusCode::BAD_REQUEST,
            "screenshot image is empty or too large",
        )
            .into_response();
    }
    match store_uploaded_screenshot(&state, server_id, query, content_type, body).await {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(ScreenshotUploadError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(ScreenshotUploadError::Invalid(message)) => {
            (StatusCode::BAD_REQUEST, message).into_response()
        }
        Err(ScreenshotUploadError::Internal(err)) => {
            tracing::warn!(error = %err, "failed to store uploaded screenshot");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn enqueue_auto_screenshot_for_deployment(
    state: &AppState,
    deployment_id: Uuid,
) -> anyhow::Result<Option<Uuid>> {
    let Some(row) = sqlx::query(
        "SELECT a.id AS app_id, a.server_id, a.domain, a.public_exposure, d.status
         FROM deployments d
         JOIN apps a ON a.id=d.app_id
         WHERE d.id=$1 AND a.current_deployment_id=$1",
    )
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(None);
    };
    let status = row.get::<String, _>("status");
    if !LIVE_DEPLOYMENT_STATUSES.contains(&status.as_str())
        || !row.get::<bool, _>("public_exposure")
    {
        return Ok(None);
    }
    let candidate = ScreenshotAutoCaptureCandidate {
        app_id: row.get::<Uuid, _>("app_id"),
        deployment_id,
        server_id: row.get("server_id"),
        domain: row.get::<String, _>("domain"),
    };
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM app_screenshots
           WHERE app_id=$1 AND deployment_id=$2 AND source=$3
         )",
    )
    .bind(candidate.app_id)
    .bind(deployment_id)
    .bind(GENERATED_SOURCE)
    .fetch_one(&state.db)
    .await?;
    if existing
        || !state
            .screenshot_hooks
            .allow_auto_capture(state, &candidate)
            .await?
        || screenshot_job_exists(state, candidate.app_id, deployment_id).await?
    {
        return Ok(None);
    }
    let job_id = enqueue_screenshot_job(
        state,
        candidate.server_id,
        candidate.app_id,
        deployment_id,
        &candidate.domain,
        30,
    )
    .await?;
    Ok(Some(job_id))
}

async fn enqueue_screenshot_for_owner(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
) -> Result<Uuid, ScreenshotQueueError> {
    let row = sqlx::query(
        "SELECT a.server_id, a.domain, a.public_exposure,
                d.id AS deployment_id, d.status
         FROM apps a
         LEFT JOIN deployments d ON d.id=a.current_deployment_id
         WHERE a.id=$1 AND a.user_id=$2",
    )
    .bind(app_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|err| ScreenshotQueueError::Internal(err.into()))?;
    let Some(row) = row else {
        return Err(ScreenshotQueueError::NotFound);
    };
    if !row.get::<bool, _>("public_exposure") {
        return Err(ScreenshotQueueError::NotReady(
            "app must be public before Hostlet can publish a screenshot".into(),
        ));
    }
    let Some(deployment_id) = row.get::<Option<Uuid>, _>("deployment_id") else {
        return Err(ScreenshotQueueError::NotReady(
            "app does not have a current deployment".into(),
        ));
    };
    let status = row.get::<Option<String>, _>("status");
    if !status
        .as_deref()
        .is_some_and(|status| LIVE_DEPLOYMENT_STATUSES.contains(&status))
    {
        return Err(ScreenshotQueueError::NotReady(
            "app must have a live deployment before capture".into(),
        ));
    }
    enqueue_screenshot_job(
        state,
        row.get("server_id"),
        app_id,
        deployment_id,
        &row.get::<String, _>("domain"),
        20,
    )
    .await
    .map_err(ScreenshotQueueError::Internal)
}

async fn enqueue_screenshot_job(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
    deployment_id: Uuid,
    domain: &str,
    priority: i32,
) -> anyhow::Result<Uuid> {
    let capture_url = capture_url_for_domain(domain);
    let payload = json!({
        "type": "capture_screenshot",
        "app_id": app_id,
        "deployment_id": deployment_id,
        "capture_url": capture_url,
        "width": 1280,
        "height": 720,
        "format": "jpeg",
        "screenshotter_image": screenshotter_image_ref()
    });
    deploy::enqueue_agent_job(
        state,
        server_id,
        Some(app_id),
        Some(deployment_id),
        "capture_screenshot",
        payload,
        priority,
    )
    .await
}

async fn screenshot_job_exists(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM agent_jobs
           WHERE app_id=$1 AND deployment_id=$2 AND job_type='capture_screenshot'
             AND status = ANY($3)
         )",
    )
    .bind(app_id)
    .bind(deployment_id)
    .bind(ACTIVE_JOB_STATUSES)
    .fetch_one(&state.db)
    .await?)
}

async fn latest_screenshot_for_owner(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row = sqlx::query(
        "SELECT s.*
         FROM app_screenshots s
         JOIN apps a ON a.id=s.app_id
         WHERE s.app_id=$1 AND a.user_id=$2
         ORDER BY s.captured_at DESC
         LIMIT 1",
    )
    .bind(app_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|row| screenshot_json(state, &row)))
}

async fn store_uploaded_screenshot(
    state: &AppState,
    server_id: Uuid,
    query: ScreenshotUploadQuery,
    content_type: &'static str,
    body: Bytes,
) -> Result<serde_json::Value, ScreenshotUploadError> {
    let authorized: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1
           FROM agent_jobs j
           JOIN deployments d ON d.id=j.deployment_id
           WHERE j.id=$1 AND j.server_id=$2 AND j.app_id=$3 AND j.deployment_id=$4
             AND j.job_type='capture_screenshot'
             AND j.status IN ('claimed','running')
             AND d.server_id=$2
         )",
    )
    .bind(query.job_id)
    .bind(server_id)
    .bind(query.app_id)
    .bind(query.deployment_id)
    .fetch_one(&state.db)
    .await
    .map_err(|err| ScreenshotUploadError::Internal(err.into()))?;
    if !authorized {
        return Err(ScreenshotUploadError::NotFound);
    }
    validate_dimensions(query.width, query.height)?;
    tokio::fs::create_dir_all(&state.screenshot_dir)
        .await
        .map_err(|err| ScreenshotUploadError::Internal(err.into()))?;
    let id = Uuid::new_v4();
    let extension = if content_type == "image/webp" {
        "webp"
    } else {
        "jpg"
    };
    let storage_path = format!("{id}.{extension}");
    let final_path = state.screenshot_dir.join(&storage_path);
    let tmp_path = state.screenshot_dir.join(format!("{id}.tmp"));
    storage::write_screenshot_file(&tmp_path, &final_path, &body)
        .await
        .map_err(|err| ScreenshotUploadError::Internal(err.into()))?;
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|err| ScreenshotUploadError::Internal(err.into()))?;
    let row = match sqlx::query(
        "INSERT INTO app_screenshots
           (id,app_id,deployment_id,agent_job_id,source,content_type,byte_size,width,height,storage_path,capture_url)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
         RETURNING *",
    )
    .bind(id)
    .bind(query.app_id)
    .bind(query.deployment_id)
    .bind(query.job_id)
    .bind(GENERATED_SOURCE)
    .bind(content_type)
    .bind(body.len() as i32)
    .bind(query.width)
    .bind(query.height)
    .bind(storage_path)
    .bind(query.capture_url)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(err) => {
            let _ = tokio::fs::remove_file(&final_path).await;
            return Err(ScreenshotUploadError::Internal(err.into()));
        }
    };
    let stored = StoredScreenshot {
        id,
        app_id: query.app_id,
        deployment_id: query.deployment_id,
        agent_job_id: query.job_id,
        public_url: public_screenshot_url(state, id),
        captured_at: row.get("captured_at"),
    };
    if let Err(err) = state
        .screenshot_hooks
        .after_screenshot_stored(&mut tx, &stored)
        .await
    {
        let _ = tx.rollback().await;
        let _ = tokio::fs::remove_file(&final_path).await;
        return Err(ScreenshotUploadError::Internal(err));
    }
    if let Err(err) = tx.commit().await {
        let _ = tokio::fs::remove_file(&final_path).await;
        return Err(ScreenshotUploadError::Internal(err.into()));
    }
    Ok(screenshot_json(state, &row))
}

fn screenshot_json(state: &AppState, row: &sqlx::postgres::PgRow) -> serde_json::Value {
    let id = row.get::<Uuid, _>("id");
    json!({
        "id": id,
        "appId": row.get::<Uuid, _>("app_id"),
        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
        "agentJobId": row.get::<Option<Uuid>, _>("agent_job_id"),
        "source": row.get::<String, _>("source"),
        "contentType": row.get::<String, _>("content_type"),
        "byteSize": row.get::<i32, _>("byte_size"),
        "width": row.get::<Option<i32>, _>("width"),
        "height": row.get::<Option<i32>, _>("height"),
        "captureUrl": row.get::<Option<String>, _>("capture_url"),
        "capturedAt": row.get::<chrono::DateTime<chrono::Utc>, _>("captured_at"),
        "publicUrl": public_screenshot_url(state, id),
    })
}

pub fn public_screenshot_url(state: &AppState, id: Uuid) -> String {
    format!(
        "{}/api/public/screenshots/{id}",
        state.public_api_url.trim_end_matches('/')
    )
}

fn screenshot_content_type(headers: &HeaderMap) -> Option<&'static str> {
    let value = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())?
        .split(';')
        .next()?
        .trim();
    match value {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

fn validate_dimensions(
    width: Option<i32>,
    height: Option<i32>,
) -> Result<(), ScreenshotUploadError> {
    let valid = |value: Option<i32>| value.is_none_or(|value| (1..=4096).contains(&value));
    if valid(width) && valid(height) {
        Ok(())
    } else {
        Err(ScreenshotUploadError::Invalid(
            "screenshot dimensions are out of range".into(),
        ))
    }
}

fn capture_url_for_domain(domain: &str) -> String {
    let host = domain.trim();
    let scheme = if host.starts_with("localhost")
        || host.starts_with("127.0.0.1")
        || host.starts_with("[::1]")
    {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{host}/")
}

fn screenshotter_image_ref() -> String {
    if let Ok(image) = std::env::var("HOSTLET_SCREENSHOTTER_IMAGE") {
        let image = image.trim();
        if !image.is_empty() {
            return image.to_string();
        }
    }
    let registry = std::env::var("HOSTLET_IMAGE_REGISTRY").unwrap_or_else(|_| "local".into());
    let tag = std::env::var("HOSTLET_IMAGE_TAG").unwrap_or_else(|_| "latest".into());
    format!(
        "{}/hostlet-screenshotter:{tag}",
        registry.trim_end_matches('/')
    )
}

enum ScreenshotQueueError {
    NotFound,
    NotReady(String),
    Internal(anyhow::Error),
}

enum ScreenshotUploadError {
    NotFound,
    Invalid(String),
    Internal(anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::extract::Path;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn capture_url_uses_https_for_public_domains() {
        assert_eq!(
            capture_url_for_domain("demo.example.com"),
            "https://demo.example.com/"
        );
    }

    #[test]
    fn capture_url_uses_http_for_localhost() {
        assert_eq!(
            capture_url_for_domain("localhost:3000"),
            "http://localhost:3000/"
        );
    }

    #[test]
    fn screenshot_content_type_accepts_jpeg_with_parameters() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("image/jpeg; charset=binary"),
        );
        assert_eq!(screenshot_content_type(&headers), Some("image/jpeg"));
    }

    #[test]
    fn screenshot_content_type_rejects_png() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        assert_eq!(screenshot_content_type(&headers), None);
    }

    #[test]
    fn validate_dimensions_bounds_uploaded_images() {
        assert!(validate_dimensions(Some(1280), Some(720)).is_ok());
        assert!(matches!(
            validate_dimensions(Some(4097), Some(720)),
            Err(ScreenshotUploadError::Invalid(_))
        ));
    }

    #[test]
    fn screenshotter_image_can_be_overridden_exactly() {
        std::env::set_var(
            "HOSTLET_SCREENSHOTTER_IMAGE",
            "registry.example.test/hostlet-screenshotter:stable",
        );
        assert_eq!(
            screenshotter_image_ref(),
            "registry.example.test/hostlet-screenshotter:stable"
        );
        std::env::remove_var("HOSTLET_SCREENSHOTTER_IMAGE");
    }

    #[tokio::test]
    async fn db_screenshot_capture_route_enqueues_core_job() {
        let Some(state) = db_test_state().await else {
            return;
        };
        reset_screenshot_db(&state).await;
        let user_id = insert_user(&state, 5101, "shot-owner").await;
        let app_id = insert_public_app(&state, user_id, "route.example.test").await;
        insert_successful_deployment(&state, app_id).await;

        let response = capture_app_screenshot(
            State(state.clone()),
            user_headers(&state, user_id),
            Path(app_id),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let job_type: String =
            sqlx::query_scalar("SELECT job_type FROM agent_jobs WHERE app_id=$1")
                .bind(app_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(job_type, "capture_screenshot");
    }

    #[tokio::test]
    async fn db_auto_capture_respects_hook_denial() {
        let Some(state) = db_test_state().await else {
            return;
        };
        reset_screenshot_db(&state).await;
        let hooks = Arc::new(RecordingHooks {
            allow: false,
            fail_after_store: false,
            stored: Mutex::new(Vec::new()),
        });
        let state = state.with_screenshot_hooks(hooks);
        let user_id = insert_user(&state, 5102, "auto-denied").await;
        let app_id = insert_public_app(&state, user_id, "denied.example.test").await;
        let deployment_id = insert_successful_deployment(&state, app_id).await;

        assert_eq!(
            enqueue_auto_screenshot_for_deployment(&state, deployment_id)
                .await
                .unwrap(),
            None
        );
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_jobs WHERE app_id=$1")
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(jobs, 0);
    }

    #[tokio::test]
    async fn db_agent_upload_stores_serves_and_scopes_latest_screenshot() {
        let Some(state) = db_test_state().await else {
            return;
        };
        reset_screenshot_db(&state).await;
        let owner_id = insert_user(&state, 5103, "shot-latest").await;
        let other_id = insert_user(&state, 5104, "shot-other").await;
        let app_id = insert_public_app(&state, owner_id, "latest.example.test").await;
        let deployment_id = insert_successful_deployment(&state, app_id).await;
        let job_id = insert_screenshot_job(&state, app_id, deployment_id, "running").await;

        let upload = upload_agent_screenshot(
            State(state.clone()),
            agent_headers(&state, "image/jpeg; charset=binary"),
            Query(ScreenshotUploadQuery {
                app_id,
                deployment_id,
                job_id,
                width: Some(1280),
                height: Some(720),
                capture_url: Some("https://latest.example.test/".into()),
            }),
            Bytes::from_static(b"fake-jpeg"),
        )
        .await;
        assert_eq!(upload.status(), StatusCode::CREATED);

        let screenshot_id: Uuid =
            sqlx::query_scalar("SELECT id FROM app_screenshots WHERE app_id=$1")
                .bind(app_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        let public = public_screenshot(State(state.clone()), Path(screenshot_id)).await;
        assert_eq!(public.status(), StatusCode::OK);
        assert_eq!(
            public.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        let bytes = to_bytes(public.into_body(), 1024).await.unwrap();
        assert_eq!(&bytes[..], b"fake-jpeg");

        let latest = latest_app_screenshot(
            State(state.clone()),
            user_headers(&state, owner_id),
            Path(app_id),
        )
        .await;
        assert_eq!(latest.status(), StatusCode::OK);
        let latest_body = to_bytes(latest.into_body(), 4096).await.unwrap();
        let latest_json: serde_json::Value = serde_json::from_slice(&latest_body).unwrap();
        assert_eq!(latest_json["id"], screenshot_id.to_string());

        let other_latest = latest_app_screenshot(
            State(state.clone()),
            user_headers(&state, other_id),
            Path(app_id),
        )
        .await;
        assert_eq!(other_latest.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn db_agent_upload_rolls_back_when_hook_fails() {
        let Some(state) = db_test_state().await else {
            return;
        };
        reset_screenshot_db(&state).await;
        let hooks = Arc::new(RecordingHooks {
            allow: true,
            fail_after_store: true,
            stored: Mutex::new(Vec::new()),
        });
        let state = state.with_screenshot_hooks(hooks);
        let user_id = insert_user(&state, 5105, "shot-rollback").await;
        let app_id = insert_public_app(&state, user_id, "rollback.example.test").await;
        let deployment_id = insert_successful_deployment(&state, app_id).await;
        let job_id = insert_screenshot_job(&state, app_id, deployment_id, "running").await;

        let upload = upload_agent_screenshot(
            State(state.clone()),
            agent_headers(&state, "image/jpeg"),
            Query(ScreenshotUploadQuery {
                app_id,
                deployment_id,
                job_id,
                width: Some(1280),
                height: Some(720),
                capture_url: Some("https://rollback.example.test/".into()),
            }),
            Bytes::from_static(b"fake-jpeg"),
        )
        .await;

        assert_eq!(upload.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM app_screenshots WHERE app_id=$1")
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        assert!(tokio::fs::read_dir(&state.screenshot_dir)
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none());
    }

    struct RecordingHooks {
        allow: bool,
        fail_after_store: bool,
        stored: Mutex<Vec<StoredScreenshot>>,
    }

    #[async_trait]
    impl ScreenshotHooks for RecordingHooks {
        async fn allow_auto_capture(
            &self,
            _state: &AppState,
            _candidate: &ScreenshotAutoCaptureCandidate,
        ) -> anyhow::Result<bool> {
            Ok(self.allow)
        }

        async fn after_screenshot_stored(
            &self,
            _tx: &mut Transaction<'_, Postgres>,
            stored: &StoredScreenshot,
        ) -> anyhow::Result<()> {
            self.stored.lock().await.push(stored.clone());
            if self.fail_after_store {
                anyhow::bail!("forced hook failure");
            }
            Ok(())
        }
    }

    async fn db_test_state() -> Option<AppState> {
        let mut state = crate::state::db_test_state_from_env().await?;
        state.screenshot_dir =
            std::env::temp_dir().join(format!("hostlet-core-screenshots-{}", Uuid::new_v4()));
        Some(state)
    }

    async fn reset_screenshot_db(state: &AppState) {
        let _ = tokio::fs::remove_dir_all(&state.screenshot_dir).await;
        tokio::fs::create_dir_all(&state.screenshot_dir)
            .await
            .unwrap();
        sqlx::query(
            "TRUNCATE app_screenshots, agent_jobs, deployments, app_env_vars, apps, users CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn insert_user(state: &AppState, github_id: i64, login: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (github_id, login) VALUES ($1,$2) RETURNING id")
            .bind(github_id)
            .bind(login)
            .fetch_one(&state.db)
            .await
            .unwrap()
    }

    async fn insert_public_app(state: &AppState, user_id: Uuid, domain: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,'screenshot-app','hostlet-ci/node-hello','main',3000,'/health',$3,'single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(state.local_server_id)
        .bind(domain)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_successful_deployment(state: &AppState, app_id: Uuid) -> Uuid {
        let deployment_id = sqlx::query_scalar(
            "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind,container_name,published_port)
             VALUES ($1,$2,'success','HEAD',now(),now(),'single',$3,32001)
             RETURNING id",
        )
        .bind(app_id)
        .bind(state.local_server_id)
        .bind(format!("hostlet-app-{app_id}"))
        .fetch_one(&state.db)
        .await
        .unwrap();
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(deployment_id)
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();
        deployment_id
    }

    async fn insert_screenshot_job(
        state: &AppState,
        app_id: Uuid,
        deployment_id: Uuid,
        status: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json)
             VALUES ($1,$2,$3,'capture_screenshot',$4,'{\"type\":\"capture_screenshot\"}'::jsonb)
             RETURNING id",
        )
        .bind(state.local_server_id)
        .bind(app_id)
        .bind(deployment_id)
        .bind(status)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    fn user_headers(state: &AppState, user_id: Uuid) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            crate::auth::test_session_cookie_header(state, user_id)
                .parse()
                .unwrap(),
        );
        headers
    }

    fn agent_headers(state: &AppState, content_type: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hostlet-server-id",
            state.local_server_id.to_string().parse().unwrap(),
        );
        headers.insert(
            "x-hostlet-agent-token",
            std::env::var("LOCAL_AGENT_TOKEN")
                .unwrap_or_else(|_| "ci-only-not-a-secret-agent-token-01".into())
                .parse()
                .unwrap(),
        );
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers
    }

    #[tokio::test]
    async fn db_sweep_spares_row_backed_files_and_removes_orphans() {
        let Some(state) = db_test_state().await else {
            return;
        };
        reset_screenshot_db(&state).await;
        let user_id = insert_user(&state, 5120, "sweep-user").await;
        let app_id = insert_public_app(&state, user_id, "sweep.example.test").await;
        let deployment_id = insert_successful_deployment(&state, app_id).await;
        let job_id = insert_screenshot_job(&state, app_id, deployment_id, "running").await;

        let backed_id = Uuid::new_v4();
        let backed_path = format!("{backed_id}.jpg");
        let orphan_id = Uuid::new_v4();
        let orphan_path = format!("{orphan_id}.jpg");

        tokio::fs::write(state.screenshot_dir.join(&backed_path), b"x")
            .await
            .unwrap();
        tokio::fs::write(state.screenshot_dir.join(&orphan_path), b"x")
            .await
            .unwrap();

        // Insert a DB row only for the backed file.
        sqlx::query(
            "INSERT INTO app_screenshots
               (id,app_id,deployment_id,agent_job_id,source,content_type,byte_size,storage_path,capture_url)
             VALUES ($1,$2,$3,$4,'generated','image/jpeg',1,$5,'u://x')",
        )
        .bind(backed_id)
        .bind(app_id)
        .bind(deployment_id)
        .bind(job_id)
        .bind(&backed_path)
        .execute(&state.db)
        .await
        .unwrap();

        // Back-date both files beyond the 1-hour threshold so the sweep considers them.
        let two_hours_ago = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(7200),
        );
        filetime::set_file_mtime(state.screenshot_dir.join(&backed_path), two_hours_ago).unwrap();
        filetime::set_file_mtime(state.screenshot_dir.join(&orphan_path), two_hours_ago).unwrap();

        super::storage::sweep_orphaned_screenshot_files(&state).await;

        assert!(
            state.screenshot_dir.join(&backed_path).exists(),
            "row-backed file must survive the sweep"
        );
        assert!(
            !state.screenshot_dir.join(&orphan_path).exists(),
            "orphaned file must be removed by the sweep"
        );
    }
}
