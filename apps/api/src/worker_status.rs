//! Process-local status tracking for long-lived background workers.
//!
//! The registry deliberately lives in memory: its records describe workers in
//! this API process, so a restarted process must register and complete its own
//! work instead of inheriting a stale success from durable storage. Registering
//! a worker and updating a run are synchronous, bounded map operations so task
//! startup can publish `Starting` before the task is spawned.

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Starting,
    Running,
    Succeeded,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug)]
pub struct WorkerSpec {
    name: &'static str,
    cadence: Duration,
    stale_after: Duration,
}

impl WorkerSpec {
    pub const fn new(name: &'static str, cadence: Duration, stale_after: Duration) -> Self {
        Self {
            name,
            cadence,
            stale_after,
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStatusSnapshot {
    pub name: String,
    pub state: WorkerState,
    pub cadence_seconds: u64,
    pub stale_after_seconds: u64,
    pub registered_at: DateTime<Utc>,
    pub last_run_started_at: Option<DateTime<Utc>>,
    pub last_run_finished_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_success_age_seconds: Option<u64>,
    pub consecutive_failures: u64,
    pub last_failure_code: Option<String>,
    pub last_result: Option<Value>,
    pub fresh: bool,
}

#[derive(Clone, Default)]
pub struct WorkerStatusRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

#[derive(Clone)]
pub struct WorkerReporter {
    registry: WorkerStatusRegistry,
    spec: WorkerSpec,
    generation: u64,
}

#[derive(Default)]
struct RegistryInner {
    next_generation: u64,
    records: BTreeMap<&'static str, WorkerRecord>,
}

struct WorkerRecord {
    generation: u64,
    spec: WorkerSpec,
    state: WorkerState,
    registered_at: DateTime<Utc>,
    last_run_started_at: Option<DateTime<Utc>>,
    last_run_finished_at: Option<DateTime<Utc>>,
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    consecutive_failures: u64,
    last_failure_code: Option<String>,
    last_result: Option<Value>,
}

impl WorkerStatusRegistry {
    pub fn register(&self, spec: WorkerSpec) -> WorkerReporter {
        self.register_at(spec, Utc::now())
    }

    pub fn register_at(&self, spec: WorkerSpec, now: DateTime<Utc>) -> WorkerReporter {
        let mut inner = self.inner();
        inner.next_generation = inner.next_generation.saturating_add(1);
        let generation = inner.next_generation;
        inner.records.insert(
            spec.name,
            WorkerRecord {
                generation,
                spec,
                state: WorkerState::Starting,
                registered_at: now,
                last_run_started_at: None,
                last_run_finished_at: None,
                last_success_at: None,
                last_failure_at: None,
                consecutive_failures: 0,
                last_failure_code: None,
                last_result: None,
            },
        );
        WorkerReporter {
            registry: self.clone(),
            spec,
            generation,
        }
    }

    pub fn snapshot(&self, name: &str) -> Option<WorkerStatusSnapshot> {
        self.snapshot_at(name, Utc::now())
    }

    pub fn snapshot_at(&self, name: &str, now: DateTime<Utc>) -> Option<WorkerStatusSnapshot> {
        self.inner()
            .records
            .get(name)
            .map(|record| record.snapshot(now))
    }

    /// Spawn a long-lived worker and record an unexpected return or panic.
    ///
    /// The closure receives the same reporter that was synchronously
    /// registered as `Starting`. It should report individual run boundaries.
    /// This primitive treats every return as unexpected; graceful shutdown is
    /// process-wide and does not need to leave a healthy worker record behind.
    pub fn spawn_supervised<Task, TaskFuture>(&self, spec: WorkerSpec, task: Task) -> JoinHandle<()>
    where
        Task: FnOnce(WorkerReporter) -> TaskFuture + Send + 'static,
        TaskFuture: Future<Output = ()> + Send + 'static,
    {
        let reporter = self.register(spec);
        tokio::spawn(async move {
            let task_reporter = reporter.clone();
            let outcome = AssertUnwindSafe(async move { task(task_reporter).await })
                .catch_unwind()
                .await;
            match outcome {
                Ok(()) => reporter.stopped("unexpected_return"),
                Err(_) => reporter.stopped("panic"),
            }
        })
    }

    fn inner(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl WorkerReporter {
    pub const fn name(&self) -> &'static str {
        self.spec.name
    }

    pub fn run_started(&self) {
        self.run_started_at(Utc::now());
    }

    pub fn run_started_at(&self, now: DateTime<Utc>) {
        self.update(|record| {
            record.state = WorkerState::Running;
            record.last_run_started_at = Some(now);
            record.last_run_finished_at = None;
        });
    }

    pub fn run_succeeded(&self, result: Option<Value>) {
        self.run_succeeded_at(Utc::now(), result);
    }

    pub fn run_succeeded_at(&self, now: DateTime<Utc>, result: Option<Value>) {
        self.update(|record| {
            record.state = WorkerState::Succeeded;
            record.last_run_finished_at = Some(now);
            record.last_success_at = Some(now);
            record.consecutive_failures = 0;
            record.last_failure_code = None;
            record.last_result = result;
        });
    }

    pub fn run_failed(&self, failure_code: impl Into<String>, result: Option<Value>) {
        self.run_failed_at(Utc::now(), failure_code, result);
    }

    pub fn run_failed_at(
        &self,
        now: DateTime<Utc>,
        failure_code: impl Into<String>,
        result: Option<Value>,
    ) {
        let failure_code = failure_code.into();
        self.update(|record| {
            record.state = WorkerState::Failed;
            record.last_run_finished_at = Some(now);
            record.last_failure_at = Some(now);
            record.consecutive_failures = record.consecutive_failures.saturating_add(1);
            record.last_failure_code = Some(failure_code);
            record.last_result = result;
        });
    }

    fn stopped(&self, failure_code: &'static str) {
        let now = Utc::now();
        self.update(|record| {
            record.state = WorkerState::Stopped;
            record.last_run_finished_at = Some(now);
            record.last_failure_at = Some(now);
            record.consecutive_failures = record.consecutive_failures.saturating_add(1);
            record.last_failure_code = Some(failure_code.to_string());
        });
    }

    fn update(&self, update: impl FnOnce(&mut WorkerRecord)) {
        if let Some(record) = self.registry.inner().records.get_mut(self.spec.name) {
            if record.generation == self.generation {
                update(record);
            }
        }
    }
}

impl WorkerRecord {
    fn snapshot(&self, now: DateTime<Utc>) -> WorkerStatusSnapshot {
        let last_success_age = self
            .last_success_at
            .and_then(|last_success| now.signed_duration_since(last_success).to_std().ok());
        let fresh = matches!(self.state, WorkerState::Running | WorkerState::Succeeded)
            && self.consecutive_failures == 0
            && last_success_age.is_some_and(|age| age <= self.spec.stale_after);
        WorkerStatusSnapshot {
            name: self.spec.name.to_string(),
            state: self.state,
            cadence_seconds: self.spec.cadence.as_secs(),
            stale_after_seconds: self.spec.stale_after.as_secs(),
            registered_at: self.registered_at,
            last_run_started_at: self.last_run_started_at,
            last_run_finished_at: self.last_run_finished_at,
            last_success_at: self.last_success_at,
            last_failure_at: self.last_failure_at,
            last_success_age_seconds: last_success_age.map(|age| age.as_secs()),
            consecutive_failures: self.consecutive_failures,
            last_failure_code: self.last_failure_code.clone(),
            last_result: self.last_result.clone(),
            fresh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SPEC: WorkerSpec = WorkerSpec::new(
        "test_worker",
        Duration::from_secs(30),
        Duration::from_secs(90),
    );

    fn instant(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + seconds, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn transitions_and_freshness_are_deterministic() {
        let registry = WorkerStatusRegistry::default();
        let reporter = registry.register_at(SPEC, instant(0));
        let starting = registry.snapshot_at(SPEC.name(), instant(0)).unwrap();
        assert_eq!(starting.state, WorkerState::Starting);
        assert!(!starting.fresh);
        assert_eq!(starting.last_success_age_seconds, None);

        reporter.run_started_at(instant(1));
        reporter.run_succeeded_at(instant(2), Some(serde_json::json!({"processed": 3})));
        let fresh = registry.snapshot_at(SPEC.name(), instant(92)).unwrap();
        assert_eq!(fresh.state, WorkerState::Succeeded);
        assert!(fresh.fresh, "the exact stale boundary remains fresh");
        assert_eq!(fresh.last_success_age_seconds, Some(90));
        assert_eq!(fresh.consecutive_failures, 0);
        assert_eq!(fresh.last_result, Some(serde_json::json!({"processed": 3})));

        reporter.run_started_at(instant(93));
        let retrying = registry.snapshot_at(SPEC.name(), instant(93)).unwrap();
        assert_eq!(retrying.state, WorkerState::Running);
        assert!(!retrying.fresh, "the previous success is now stale");

        reporter.run_failed_at(
            instant(94),
            "partial_result",
            Some(serde_json::json!({"failed": 1})),
        );
        let failed = registry.snapshot_at(SPEC.name(), instant(94)).unwrap();
        assert_eq!(failed.state, WorkerState::Failed);
        assert!(!failed.fresh);
        assert_eq!(failed.consecutive_failures, 1);
        assert_eq!(failed.last_failure_code.as_deref(), Some("partial_result"));

        reporter.run_started_at(instant(95));
        let retrying_after_failure = registry.snapshot_at(SPEC.name(), instant(95)).unwrap();
        assert!(!retrying_after_failure.fresh);
        reporter.run_succeeded_at(instant(96), None);
        let recovered = registry.snapshot_at(SPEC.name(), instant(100)).unwrap();
        assert_eq!(recovered.state, WorkerState::Succeeded);
        assert!(recovered.fresh);
        assert_eq!(recovered.consecutive_failures, 0);
        assert_eq!(recovered.last_failure_code, None);
    }

    #[test]
    fn future_success_timestamp_fails_closed() {
        let registry = WorkerStatusRegistry::default();
        let reporter = registry.register_at(SPEC, instant(0));
        reporter.run_started_at(instant(10));
        reporter.run_succeeded_at(instant(20), None);

        let snapshot = registry.snapshot_at(SPEC.name(), instant(19)).unwrap();
        assert!(!snapshot.fresh);
        assert_eq!(snapshot.last_success_age_seconds, None);
    }

    #[test]
    fn replaced_worker_ignores_updates_from_old_reporter() {
        let registry = WorkerStatusRegistry::default();
        let old = registry.register_at(SPEC, instant(0));
        let current = registry.register_at(SPEC, instant(10));

        old.run_started_at(instant(11));
        old.run_succeeded_at(instant(12), Some(serde_json::json!({"old": true})));
        let still_starting = registry.snapshot_at(SPEC.name(), instant(12)).unwrap();
        assert_eq!(still_starting.state, WorkerState::Starting);
        assert_eq!(still_starting.registered_at, instant(10));
        assert_eq!(still_starting.last_result, None);

        current.run_started_at(instant(13));
        current.run_succeeded_at(instant(14), Some(serde_json::json!({"current": true})));
        let succeeded = registry.snapshot_at(SPEC.name(), instant(14)).unwrap();
        assert_eq!(succeeded.state, WorkerState::Succeeded);
        assert_eq!(
            succeeded.last_result,
            Some(serde_json::json!({"current": true}))
        );
    }

    #[tokio::test]
    async fn supervisor_records_unexpected_return() {
        let registry = WorkerStatusRegistry::default();
        let handle = registry.spawn_supervised(SPEC, |_reporter| async {});
        handle.await.unwrap();

        let snapshot = registry.snapshot(SPEC.name()).unwrap();
        assert_eq!(snapshot.state, WorkerState::Stopped);
        assert!(!snapshot.fresh);
        assert_eq!(snapshot.consecutive_failures, 1);
        assert_eq!(
            snapshot.last_failure_code.as_deref(),
            Some("unexpected_return")
        );
    }

    #[tokio::test]
    async fn supervisor_records_panic() {
        let registry = WorkerStatusRegistry::default();
        let handle = registry.spawn_supervised(SPEC, |_reporter| async {
            panic!("test worker panic");
        });
        handle.await.unwrap();

        let snapshot = registry.snapshot(SPEC.name()).unwrap();
        assert_eq!(snapshot.state, WorkerState::Stopped);
        assert!(!snapshot.fresh);
        assert_eq!(snapshot.consecutive_failures, 1);
        assert_eq!(snapshot.last_failure_code.as_deref(), Some("panic"));
    }
}
