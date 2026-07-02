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
    let job_type: String = sqlx::query_scalar("SELECT job_type FROM agent_jobs WHERE app_id=$1")
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

    let screenshot_id: Uuid = sqlx::query_scalar("SELECT id FROM app_screenshots WHERE app_id=$1")
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let public =
        public_screenshot(State(state.clone()), HeaderMap::new(), Path(screenshot_id)).await;
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(
        public.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    let etag = public.headers().get(header::ETAG).unwrap().clone();
    let bytes = to_bytes(public.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"fake-jpeg");

    let mut conditional_headers = HeaderMap::new();
    conditional_headers.insert(header::IF_NONE_MATCH, etag);
    let cached = public_screenshot(
        State(state.clone()),
        conditional_headers,
        Path(screenshot_id),
    )
    .await;
    assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
    assert!(to_bytes(cached.into_body(), 1024).await.unwrap().is_empty());

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
        _state: &AppState,
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

#[tokio::test]
async fn db_recapture_sweep_enqueues_for_stale_screenshot_and_skips_fresh() {
    let Some(state) = db_test_state().await else {
        return;
    };
    reset_screenshot_db(&state).await;
    let user_id = insert_user(&state, 5130, "recapture-user").await;
    let stale_app_id = insert_public_app(&state, user_id, "stale.example.test").await;
    let stale_deployment_id = insert_successful_deployment(&state, stale_app_id).await;
    let fresh_app_id = insert_public_app(&state, user_id, "fresh.example.test").await;
    let fresh_deployment_id = insert_successful_deployment(&state, fresh_app_id).await;

    insert_screenshot_row_with_age(&state, stale_app_id, stale_deployment_id, 40).await;
    insert_screenshot_row_with_age(&state, fresh_app_id, fresh_deployment_id, 1).await;

    super::recapture::sweep_stale_screenshots_for_test(&state).await;

    let stale_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_jobs WHERE app_id=$1 AND job_type='capture_screenshot'",
    )
    .bind(stale_app_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(stale_jobs, 1, "stale app should get a recapture job");

    let fresh_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_jobs WHERE app_id=$1 AND job_type='capture_screenshot'",
    )
    .bind(fresh_app_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(fresh_jobs, 0, "fresh app should not be re-queued");
}

async fn insert_screenshot_row_with_age(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
    age_days: i64,
) {
    sqlx::query(
        "INSERT INTO app_screenshots
           (id,app_id,deployment_id,source,content_type,byte_size,storage_path,capture_url,captured_at)
         VALUES ($1,$2,$3,'generated','image/jpeg',1,$4,'u://x',now() - ($5 || ' days')::interval)",
    )
    .bind(Uuid::new_v4())
    .bind(app_id)
    .bind(deployment_id)
    .bind(format!("{}.jpg", Uuid::new_v4()))
    .bind(age_days.to_string())
    .execute(&state.db)
    .await
    .unwrap();
}
