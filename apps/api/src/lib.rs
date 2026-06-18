pub mod agent;
pub mod apps;
pub mod auth;
pub mod cleanup;
pub mod crypto;
pub mod deploy;
pub mod deployment_policy;
pub mod env;
pub mod github;
pub mod policies;
pub mod rate_limit;
pub mod screenshots;
pub mod server_capacity;
pub mod state;
pub mod web;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{
        header, request::Parts, HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Router,
};
use state::AppState;
use std::net::SocketAddr;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

/// Interval between background sweeps of stale deployments and agent jobs.
/// Startup runs the same recovery once, but a crashed or offline agent can
/// strand jobs/deployments at any time; without this sweep they block new
/// deploys until the API restarts.
const RUNTIME_RECOVERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

pub async fn run_from_env() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "hostlet_api=info,tower_http=info".into()),
        )
        .init();

    let state = AppState::from_env().await?;
    recover_startup_state(&state).await?;
    // One-shot orphan sweep: runs once at startup in the background to remove
    // screenshot files that have no app_screenshots row. Not added to the
    // RUNTIME_RECOVERY ticker; the sweep is startup hygiene, not periodic.
    let sweep_state = state.clone();
    tokio::spawn(async move {
        screenshots::sweep_orphaned_screenshot_files(&sweep_state).await;
    });
    let recovery_state = state.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RUNTIME_RECOVERY_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes immediately and startup recovery just ran.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            recover_runtime_state(&recovery_state).await;
        }
    });
    if state.update_checks_enabled {
        let update_state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = web::refresh_update_check_if_stale(&update_state).await {
                tracing::warn!(error = %err, "Hostlet update check failed");
            }
        });
    }

    let app = core_router(state)?;
    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    tracing::info!("api listening on {addr}");
    axum::serve(
        tokio::net::TcpListener::bind(addr).await?,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

pub async fn recover_startup_state(state: &AppState) -> anyhow::Result<()> {
    let recovered = deploy::recover_stale_deployments_and_cleanup(state).await?;
    if recovered > 0 {
        tracing::warn!(recovered, "marked stale deployments as failed");
    }
    let recovered_jobs = agent::recover_stale_agent_jobs(state).await?;
    if recovered_jobs > 0 {
        tracing::warn!(recovered_jobs, "reconciled stale agent jobs");
    }
    let finalized_deletes = web::reconcile_completed_delete_jobs(state).await?;
    if finalized_deletes > 0 {
        tracing::warn!(finalized_deletes, "finalized completed delete jobs");
    }
    Ok(())
}

/// One background recovery sweep. Both recover functions are idempotent
/// UPDATE-based sweeps shared with startup; database errors are logged and
/// never end the loop.
async fn recover_runtime_state(state: &AppState) {
    match deploy::recover_stale_deployments(state).await {
        Ok(recovered) if recovered > 0 => {
            tracing::warn!(recovered, "marked stale deployments as failed");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "periodic stale-deployment recovery failed"),
    }
    match agent::recover_stale_agent_jobs(state).await {
        Ok(recovered) if recovered > 0 => {
            tracing::warn!(recovered, "reconciled stale agent jobs");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "periodic stale-job recovery failed"),
    }
}

pub fn core_router(state: AppState) -> anyhow::Result<Router> {
    let allowed_cors_origins = state
        .allowed_web_origins
        .iter()
        .map(|origin| {
            origin
                .parse::<HeaderValue>()
                .map_err(|err| anyhow::anyhow!("{origin} is not a valid CORS origin: {err}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let guard_state = state.clone();
    Ok(Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/install-agent.sh",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/x-shellscript")],
                    include_str!("../../../scripts/install-agent.sh"),
                )
            }),
        )
        .route("/auth/github/device/start", post(auth::github_device_start))
        .route("/auth/github/device/poll", post(auth::github_device_poll))
        .route("/api/session", get(auth::session_status))
        .route("/api/setup/status", get(auth::setup_status))
        .route("/api/setup", post(auth::setup_password))
        .route("/api/unlock", post(auth::unlock))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/github/status", get(github::status))
        .route("/api/github/repos", get(github::repos))
        .route("/api/github/repo-inspect", post(github::repo_inspect))
        .route("/api/addons", get(web::addons_catalog))
        .route("/api/cloudflare/status", get(web::cloudflare_status))
        .route("/api/system/version", get(web::system_version))
        .route("/api/system/backups/latest", get(web::backup_metadata))
        .route("/api/system/update-check", post(web::system_update_check))
        .route("/api/system/operator-status", get(web::operator_status))
        .route(
            "/api/system/operator-cleanup",
            get(web::operator_cleanup_preview).post(web::operator_run_cleanup),
        )
        .route(
            "/api/system/cleanup",
            get(web::cleanup_preview).post(web::run_cleanup),
        )
        .route("/api/audit-events", get(web::audit_events))
        .route(
            "/api/servers",
            get(web::list_servers).post(web::create_server),
        )
        .route("/api/servers/:id/install", get(web::server_install_command))
        .route("/api/agent/register", post(agent::register))
        .route("/api/agent/events", post(agent::event))
        .route("/api/agent/health-targets", get(agent::health_targets))
        .route(
            "/api/agent/screenshots",
            post(screenshots::upload_agent_screenshot),
        )
        .route("/api/agent/jobs/claim", post(agent::claim_job))
        .route("/api/agent/jobs/:id/complete", post(agent::complete_job))
        .route("/api/apps", get(web::list_apps).post(web::create_app))
        .route(
            "/api/apps/:id",
            get(web::get_app)
                .patch(web::update_app)
                .delete(web::delete_app),
        )
        .route("/api/apps/:id/resources", get(web::app_resources))
        .route("/api/apps/:id/health", get(web::app_health))
        .route("/api/apps/:id/health/events", get(web::app_health_events))
        .route(
            "/api/apps/:id/health/check-now",
            post(web::check_app_health_now),
        )
        .route("/api/apps/:id/restart", post(web::restart_app_container))
        .route(
            "/api/apps/:id/screenshots",
            post(screenshots::capture_app_screenshot),
        )
        .route(
            "/api/apps/:id/screenshots/latest",
            get(screenshots::latest_app_screenshot),
        )
        .route("/api/health/summary", get(web::health_summary))
        .route("/api/apps/:id/env", get(web::app_env_vars))
        .route(
            "/api/apps/:id/env/:key",
            put(web::set_app_env_var).delete(web::delete_app_env_var),
        )
        .route("/api/apps/:id/deploy", post(deploy::manual_deploy))
        .route("/api/apps/:id/rollback", post(deploy::rollback))
        .route("/api/agent-jobs", get(web::list_agent_jobs))
        .route("/api/agent-jobs/:id", get(web::agent_job_status))
        .route("/api/agent-jobs/:id/retry", post(web::retry_agent_job))
        .route("/api/agent-jobs/:id/cancel", post(web::cancel_agent_job))
        .route("/api/deployments/:id", get(deploy::get_deployment))
        .route("/api/deployments/:id/logs", get(deploy::deployment_logs))
        .route(
            "/api/public/screenshots/:id",
            get(screenshots::public_screenshot),
        )
        .route("/ws/agent", get(agent::ws))
        .route("/ws/logs/:deployment_id", get(deploy::logs_ws))
        .route("/webhooks/github", post(github::webhook))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(
                    move |origin: &HeaderValue, _request: &Parts| {
                        allowed_cors_origins.iter().any(|allowed| allowed == origin)
                    },
                ))
                .allow_credentials(true)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    header::CONTENT_TYPE,
                    header::AUTHORIZATION,
                    HeaderName::from_static("x-hostlet-csrf"),
                    HeaderName::from_static("x-hostlet-setup-token"),
                ]),
        )
        .layer(middleware::from_fn_with_state(
            guard_state,
            browser_origin_guard,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::rate_limit,
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn browser_origin_guard(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !requires_browser_origin(req.method(), req.uri().path()) {
        return next.run(req).await;
    }
    let headers = req.headers();
    let csrf_ok = headers
        .get("x-hostlet-csrf")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "1");
    let origin_ok = request_origin(headers)
        .as_deref()
        .is_some_and(|origin| state.web_origin_allowed(origin));
    if !csrf_ok || !origin_ok {
        return (StatusCode::FORBIDDEN, "invalid request origin").into_response();
    }
    next.run(req).await
}

/// Exact paths whose mutating requests are non-browser (first-run setup token,
/// the operator cleanup hook, and the GitHub webhook) and so skip the guard.
///
/// Keep in sync with the corresponding routes registered in [`core_router`].
const ORIGIN_GUARD_EXEMPT_PATHS: &[&str] = &[
    "/api/setup",
    "/api/system/operator-cleanup",
    "/webhooks/github",
];

fn requires_browser_origin(method: &Method, path: &str) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    ) && !is_machine_agent_path(path)
        && !ORIGIN_GUARD_EXEMPT_PATHS.contains(&path)
}

fn is_machine_agent_path(path: &str) -> bool {
    matches!(
        path,
        "/api/agent/register"
            | "/api/agent/events"
            | "/api/agent/jobs/claim"
            | "/api/agent/screenshots"
    ) || path
        .strip_prefix("/api/agent/jobs/")
        .and_then(|rest| rest.strip_suffix("/complete"))
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

fn request_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(state::normalize_origin)
        .or_else(|| {
            headers
                .get(header::REFERER)
                .and_then(|value| value.to_str().ok())
                .and_then(state::normalize_origin)
        })
}

async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_mutations_require_origin_guard() {
        assert!(requires_browser_origin(&Method::POST, "/api/apps"));
        assert!(requires_browser_origin(
            &Method::PATCH,
            "/api/apps/00000000-0000-0000-0000-000000000001"
        ));
        assert!(requires_browser_origin(
            &Method::DELETE,
            "/api/apps/00000000-0000-0000-0000-000000000001"
        ));
    }

    #[test]
    fn machine_webhooks_skip_browser_origin_guard() {
        assert!(!requires_browser_origin(&Method::POST, "/webhooks/github"));
    }

    #[test]
    fn known_machine_agent_routes_skip_browser_origin_guard() {
        assert!(!requires_browser_origin(
            &Method::POST,
            "/api/agent/register"
        ));
        assert!(!requires_browser_origin(&Method::POST, "/api/agent/events"));
        assert!(!requires_browser_origin(
            &Method::POST,
            "/api/agent/jobs/claim"
        ));
        assert!(!requires_browser_origin(
            &Method::POST,
            "/api/agent/jobs/00000000-0000-0000-0000-000000000001/complete"
        ));
    }

    #[test]
    fn unknown_agent_like_mutations_require_browser_origin_guard() {
        assert!(requires_browser_origin(&Method::POST, "/api/agent/unknown"));
        assert!(requires_browser_origin(
            &Method::POST,
            "/api/agent/jobs/not-a-uuid/complete"
        ));
        assert!(requires_browser_origin(
            &Method::POST,
            "/api/agent/jobs/00000000-0000-0000-0000-000000000001/retry"
        ));
    }

    #[test]
    fn safe_methods_skip_browser_origin_guard() {
        assert!(!requires_browser_origin(&Method::GET, "/api/apps"));
    }
}
