mod agent;
mod auth;
mod crypto;
mod deploy;
mod github;
mod state;
mod web;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "hostlet_api=info,tower_http=info".into()),
        )
        .init();

    let state = AppState::from_env().await?;
    let recovered = deploy::recover_stale_deployments(&state).await?;
    if recovered > 0 {
        tracing::warn!(recovered, "marked stale deployments as failed");
    }
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
    let app = Router::new()
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
        .route("/auth/github/start", get(auth::github_start))
        .route("/auth/github/callback", get(auth::github_callback))
        .route("/api/setup/status", get(auth::setup_status))
        .route("/api/setup", post(auth::setup_password))
        .route("/api/unlock", post(auth::unlock))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/github/status", get(github::status))
        .route("/api/github/repos", get(github::repos))
        .route("/api/cloudflare/status", get(web::cloudflare_status))
        .route(
            "/api/servers",
            get(web::list_servers).post(web::create_server),
        )
        .route("/api/servers/:id/install", get(web::server_install_command))
        .route("/api/agent/register", post(agent::register))
        .route("/api/agent/events", post(agent::event))
        .route("/api/apps", get(web::list_apps).post(web::create_app))
        .route(
            "/api/apps/:id",
            get(web::get_app)
                .patch(web::update_app)
                .delete(web::delete_app),
        )
        .route("/api/apps/:id/resources", get(web::app_resources))
        .route("/api/apps/:id/env", get(web::app_env_vars))
        .route(
            "/api/apps/:id/env/:key",
            put(web::set_app_env_var).delete(web::delete_app_env_var),
        )
        .route("/api/apps/:id/deploy", post(deploy::manual_deploy))
        .route("/api/apps/:id/rollback", post(deploy::rollback))
        .route("/api/agent-jobs/:id", get(web::agent_job_status))
        .route("/api/deployments/:id", get(deploy::get_deployment))
        .route("/api/deployments/:id/logs", get(deploy::deployment_logs))
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
        .layer(middleware::from_fn(security_headers))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    tracing::info!("api listening on {addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
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

fn requires_browser_origin(method: &Method, path: &str) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    ) && !path.starts_with("/api/agent/")
        && path != "/api/setup"
        && path != "/webhooks/github"
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
