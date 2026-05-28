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
        if buckets.len() > 10_000 {
            buckets
                .retain(|_, bucket| now.duration_since(bucket.started_at) <= bucket_window_ttl());
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
    match (method, path) {
        (&Method::POST, "/api/setup") => Some(Rule {
            name: "setup",
            max: 8,
            window: Duration::from_secs(60),
        }),
        (&Method::POST, "/api/unlock") => Some(Rule {
            name: "unlock",
            max: 10,
            window: Duration::from_secs(60),
        }),
        (&Method::POST, "/auth/github/device/start") => Some(Rule {
            name: "github-device-start",
            max: 12,
            window: Duration::from_secs(60),
        }),
        (&Method::POST, "/auth/github/device/poll") => Some(Rule {
            name: "github-device-poll",
            max: 90,
            window: Duration::from_secs(60),
        }),
        (&Method::GET, "/auth/github/oauth/start") => Some(Rule {
            name: "github-oauth-start",
            max: 20,
            window: Duration::from_secs(60),
        }),
        (&Method::GET, "/auth/github/oauth/callback") => Some(Rule {
            name: "github-oauth-callback",
            max: 40,
            window: Duration::from_secs(60),
        }),
        (&Method::POST, "/api/agent/register") => Some(Rule {
            name: "agent-register",
            max: 20,
            window: Duration::from_secs(60),
        }),
        (&Method::POST, "/api/agent/events") => Some(Rule {
            name: "agent-events",
            max: 1_500,
            window: Duration::from_secs(60),
        }),
        (&Method::GET, "/ws/agent") => Some(Rule {
            name: "agent-ws",
            max: 30,
            window: Duration::from_secs(60),
        }),
        (&Method::POST, "/webhooks/github") => Some(Rule {
            name: "github-webhook",
            max: 120,
            window: Duration::from_secs(60),
        }),
        _ => None,
    }
}

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

fn bucket_window_ttl() -> Duration {
    Duration::from_secs(5 * 60)
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
