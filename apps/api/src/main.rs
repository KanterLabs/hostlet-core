mod agent;
mod auth;
mod crypto;
mod deploy;
mod github;
mod state;
mod web;

use axum::{
    http::{header, request::Parts, HeaderValue, Method},
    routing::{get, post},
    Router,
};
use state::AppState;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
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
    let public_web_origin: HeaderValue = state
        .public_web_url
        .trim_end_matches('/')
        .parse()
        .map_err(|err| anyhow::anyhow!("PUBLIC_WEB_URL is not a valid CORS origin: {err}"))?;
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
        .route("/api/me", get(auth::me))
        .route("/api/github/status", get(github::status))
        .route("/api/github/repos", get(github::repos))
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
        .route("/api/apps/:id/deploy", post(deploy::manual_deploy))
        .route("/api/apps/:id/rollback", post(deploy::rollback))
        .route("/api/deployments/:id", get(deploy::get_deployment))
        .route("/api/deployments/:id/logs", get(deploy::deployment_logs))
        .route("/ws/agent", get(agent::ws))
        .route("/ws/logs/:deployment_id", get(deploy::logs_ws))
        .route("/webhooks/github", post(github::webhook))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(
                    move |origin: &HeaderValue, _request: &Parts| {
                        origin == public_web_origin || allowed_lan_origin(origin)
                    },
                ))
                .allow_credentials(true)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    tracing::info!("api listening on {addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

fn allowed_lan_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host_str() {
        Some("localhost") => true,
        Some(host) if host.ends_with(".ts.net") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .map(is_private_control_plane_ip)
            .unwrap_or(false),
        None => false,
    }
}

fn is_private_control_plane_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip == Ipv4Addr::new(100, 100, 100, 100)
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        }
        IpAddr::V6(ip) => ip.is_loopback() || is_unique_local_ipv6(ip),
    }
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}
