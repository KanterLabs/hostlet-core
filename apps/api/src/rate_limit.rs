use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

#[derive(Debug)]
struct Bucket {
    started_at: Instant,
    count: u32,
}

#[derive(Clone, Copy)]
struct Rule {
    name: &'static str,
    max: u32,
    window: Duration,
}

/// Once the bucket map grows past this many distinct keys we sweep expired
/// entries. It is a soft cap on memory, not a correctness bound: picked high
/// enough that normal traffic never triggers a sweep, low enough that a flood
/// of one-off keys can't grow the map unboundedly.
const BUCKET_EVICTION_THRESHOLD: usize = 10_000;

/// Idle buckets older than this are dropped during an eviction sweep. It only
/// needs to exceed the longest per-route window so we never evict a bucket that
/// is still inside its active window.
const BUCKET_IDLE_TTL: Duration = Duration::from_secs(5 * 60);

/// The fixed window every per-route limit below is measured over.
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Per-route fixed-window limits, evaluated top-to-bottom in [`rule_for`].
///
/// `max` is the number of requests allowed per `window` per route/client bucket.
/// Values are sized to the expected legitimate call rate of each endpoint with
/// headroom for retries: interactive auth flows (`setup`, `unlock`, device
/// start) stay low to blunt brute force; the device-poll loop and webhook/agent
/// telemetry paths run hot, so `agent-events` is by far the most permissive.
const RULES: &[(Method, &str, Rule)] = &[
    (
        Method::POST,
        "/api/setup",
        Rule {
            name: "setup",
            max: 8,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::POST,
        "/api/unlock",
        Rule {
            name: "unlock",
            max: 10,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::POST,
        "/auth/github/device/start",
        Rule {
            name: "github-device-start",
            max: 12,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::POST,
        "/auth/github/device/poll",
        Rule {
            name: "github-device-poll",
            max: 90,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::POST,
        "/api/agent/register",
        Rule {
            name: "agent-register",
            max: 20,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::POST,
        "/api/agent/events",
        Rule {
            name: "agent-events",
            max: 1_500,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::GET,
        "/ws/agent",
        Rule {
            name: "agent-ws",
            max: 30,
            window: RATE_WINDOW,
        },
    ),
    (
        Method::POST,
        "/webhooks/github",
        Rule {
            name: "github-webhook",
            max: 120,
            window: RATE_WINDOW,
        },
    ),
];

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }
}

impl RateLimiter {
    pub fn check(&self, key: String, max: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        if buckets.len() > BUCKET_EVICTION_THRESHOLD {
            buckets.retain(|_, bucket| now.duration_since(bucket.started_at) <= BUCKET_IDLE_TTL);
        }
        let bucket = buckets.entry(key).or_insert(Bucket {
            started_at: now,
            count: 0,
        });
        if now.duration_since(bucket.started_at) > window {
            bucket.started_at = now;
            bucket.count = 0;
        }
        bucket.count += 1;
        bucket.count <= max
    }
}

pub async fn rate_limit(
    State(state): State<crate::state::AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let Some(rule) = rule_for(req.method(), req.uri().path()) else {
        return next.run(req).await;
    };
    let key = rate_limit_key(
        &state,
        rule.name,
        req.uri().path(),
        addr.ip(),
        req.headers(),
    )
    .await;
    if !state.rate_limiter.check(key, rule.max, rule.window) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "too many requests; wait before retrying",
        )
            .into_response();
    }
    next.run(req).await
}

fn rule_for(method: &Method, path: &str) -> Option<Rule> {
    RULES
        .iter()
        .find(|(rule_method, rule_path, _)| rule_method == method && *rule_path == path)
        .map(|(_, _, rule)| *rule)
}

async fn rate_limit_key(
    state: &crate::state::AppState,
    name: &str,
    path: &str,
    peer_ip: IpAddr,
    headers: &HeaderMap,
) -> String {
    let client_ip = client_ip(peer_ip, headers);
    let Some(server_id) = authenticated_agent_scope(state, path, headers).await else {
        return format!("{name}:{client_ip}");
    };
    format!("{name}:{client_ip}:{server_id}")
}

async fn authenticated_agent_scope(
    state: &crate::state::AppState,
    path: &str,
    headers: &HeaderMap,
) -> Option<Uuid> {
    if !is_agent_scoped_route(path) {
        return None;
    }
    crate::agent::authenticated_server_id(state, headers).await
}

fn is_agent_scoped_route(path: &str) -> bool {
    matches!(path, "/api/agent/events" | "/ws/agent")
}

fn client_ip(peer_ip: IpAddr, headers: &HeaderMap) -> IpAddr {
    if !trusted_proxy(peer_ip) {
        return peer_ip;
    }
    forwarded_for(headers).unwrap_or(peer_ip)
}

fn trusted_proxy(peer_ip: IpAddr) -> bool {
    peer_ip.is_loopback()
}

fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .and_then(|value| value.parse::<IpAddr>().ok())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<IpAddr>().ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn limits_within_window() {
        let limiter = RateLimiter::default();
        assert!(limiter.check("a".into(), 2, Duration::from_secs(60)));
        assert!(limiter.check("a".into(), 2, Duration::from_secs(60)));
        assert!(!limiter.check("a".into(), 2, Duration::from_secs(60)));
    }

    #[test]
    fn does_not_limit_unlisted_paths() {
        assert!(rule_for(&Method::GET, "/api/apps").is_none());
    }

    #[test]
    fn unauthenticated_server_id_does_not_scope_public_buckets() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hostlet-server-id",
            "00000000-0000-0000-0000-000000000001".parse().unwrap(),
        );
        let ip = "203.0.113.10".parse().unwrap();
        assert_eq!(ip, client_ip(ip, &headers));
        assert!(!is_agent_scoped_route("/api/setup"));
        assert!(!is_agent_scoped_route("/api/unlock"));
        assert!(is_agent_scoped_route("/api/agent/events"));
    }

    #[test]
    fn trusted_loopback_proxy_uses_forwarded_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "203.0.113.10, 127.0.0.1".parse().unwrap(),
        );
        let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
            client_ip(peer, &headers)
        );
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.10".parse().unwrap());
        let peer = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));
        assert_eq!(peer, client_ip(peer, &headers));
    }
}
