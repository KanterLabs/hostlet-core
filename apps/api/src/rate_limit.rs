use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

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
/// `max` is the number of requests allowed per `window` per
/// ([`Rule::name`], client IP[, agent scope]) bucket. Values are sized to the
/// expected legitimate call rate of each endpoint with headroom for retries:
/// interactive auth flows (`setup`, `unlock`, device start) stay low to blunt
/// brute force; the device-poll loop and webhook/agent telemetry paths run hot,
/// so `agent-events` is by far the most permissive.
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
    let key = rate_limit_key(rule.name, addr.ip(), req.headers());
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

/// Builds the bucket key for a request.
///
/// Keys are always anchored on the client IP, so the floor on quota is per-IP.
/// The optional `x-hostlet-server-id` header only ever *narrows* the bucket
/// (appends a suffix), letting many distinct agents behind one NAT'd IP avoid
/// starving each other on the hot agent/webhook routes. Because the header is
/// client-supplied and unauthenticated, a forger can split their own IP's
/// counter into extra sub-buckets and thereby multiply their effective quota;
/// this is an accepted trade-off for agent fan-out and must not be relied on as
/// a security boundary. Tightening it (e.g. signing the header, or only
/// honoring it on `/api/agent/*` and `/webhooks/*`) would change bucketing
/// behavior, so it is intentionally left as-is here.
fn rate_limit_key(name: &str, ip: IpAddr, headers: &axum::http::HeaderMap) -> String {
    let agent_scope = headers
        .get("x-hostlet-server-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 64)
        .unwrap_or("");
    if agent_scope.is_empty() {
        format!("{name}:{ip}")
    } else {
        format!("{name}:{ip}:{agent_scope}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
