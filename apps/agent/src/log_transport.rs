use crate::{log, runtime::Config};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    sync::mpsc,
    task::JoinHandle,
};
use uuid::Uuid;

/// Number of lines retained for each child output stream while the API is
/// slow. Readers use `try_send`, so this is a hard memory bound and never
/// applies backpressure to the child pipe.
pub(crate) const LOG_BUFFER_CAPACITY: usize = 256;
/// Keep the command-side line bound aligned with the API's stored-line bound.
/// The API performs its own truncation too, but capping here prevents one
/// unterminated child line from growing the reader's allocation without limit.
const LOG_LINE_MAX_BYTES: usize = 8 * 1024;
/// A single API log POST must not keep a dispatcher (or shutdown) stuck.
pub(crate) const LOG_POST_TIMEOUT: Duration = Duration::from_secs(5);
/// Reader and dispatcher shutdown are best-effort after the child exits. Any
/// remaining output is explicitly dropped after this deadline so a descendant
/// holding a pipe open or a stalled API cannot deadlock a build.
pub(crate) const LOG_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

type PostFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type PostFn = Arc<dyn Fn(String) -> PostFuture + Send + Sync + 'static>;

/// A bounded, non-blocking producer for one child output stream. When the
/// queue is full, newest lines are dropped and the count is surfaced as an
/// overflow line once capacity becomes available (or when the stream closes).
#[derive(Clone)]
pub(crate) struct LogSender {
    tx: mpsc::Sender<String>,
    stream: &'static str,
    dropped: Arc<AtomicU64>,
}

impl LogSender {
    fn new(tx: mpsc::Sender<String>, stream: &'static str, dropped: Arc<AtomicU64>) -> Self {
        Self {
            tx,
            stream,
            dropped,
        }
    }

    /// Enqueue a line without waiting for the API dispatcher. A full queue is
    /// an intentional lossy backpressure policy: the child must continue
    /// writing rather than block on a deployment-log network request.
    pub(crate) fn try_send(&self, line: String) {
        let line = cap_line(&line);

        let pending = self.dropped.swap(0, Ordering::Relaxed);
        if pending > 0 {
            let marker = overflow_line(self.stream, pending);
            match self.tx.try_send(marker) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    self.dropped.fetch_add(pending, Ordering::Relaxed);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }

        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    #[cfg(test)]
    fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Owns the bounded queue and its API-posting dispatcher for one stream.
/// Production construction wraps the existing authenticated `log` helper;
/// tests can inject a sink that deliberately stalls.
pub(crate) struct LogTransport {
    sender: Option<mpsc::Sender<String>>,
    dropped: Arc<AtomicU64>,
    stream: &'static str,
    dispatcher: Option<JoinHandle<()>>,
}

impl LogTransport {
    pub(crate) fn new(cfg: Config, deployment_id: Uuid, stream: &'static str) -> Self {
        let post: PostFn = Arc::new(move |line| {
            let cfg = cfg.clone();
            Box::pin(async move {
                match tokio::time::timeout(
                    LOG_POST_TIMEOUT,
                    log(&cfg, deployment_id, stream, &line),
                )
                .await
                {
                    Ok(()) => {}
                    Err(_) => tracing::warn!(
                        deployment_id = %deployment_id,
                        stream,
                        "deployment log POST timed out"
                    ),
                }
            })
        });
        Self::new_with_post(stream, post)
    }

    fn new_with_post(stream: &'static str, post: PostFn) -> Self {
        let (tx, mut rx) = mpsc::channel(LOG_BUFFER_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let dispatcher_dropped = dropped.clone();
        let dispatcher = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                post_line(&post, line).await;
            }

            let dropped = dispatcher_dropped.swap(0, Ordering::Relaxed);
            if dropped > 0 {
                post_line(&post, overflow_line(stream, dropped)).await;
            }
        });
        Self {
            sender: Some(tx),
            dropped,
            stream,
            dispatcher: Some(dispatcher),
        }
    }

    pub(crate) fn sender(&self) -> LogSender {
        LogSender::new(
            self.sender
                .as_ref()
                .expect("log transport sender requested after shutdown")
                .clone(),
            self.stream,
            self.dropped.clone(),
        )
    }

    /// Close the queue, then wait only a bounded amount of time for queued
    /// posts. A stalled sink is aborted and any remaining lines are dropped.
    pub(crate) async fn shutdown(mut self) {
        self.sender.take();
        let Some(mut dispatcher) = self.dispatcher.take() else {
            return;
        };
        if tokio::time::timeout(LOG_SHUTDOWN_TIMEOUT, &mut dispatcher)
            .await
            .is_err()
        {
            tracing::warn!(
                stream = self.stream,
                "discarding buffered deployment logs at shutdown"
            );
            dispatcher.abort();
            let _ = dispatcher.await;
        }
    }
}

impl Drop for LogTransport {
    fn drop(&mut self) {
        if let Some(dispatcher) = self.dispatcher.take() {
            dispatcher.abort();
        }
    }
}

async fn post_line(post: &PostFn, line: String) {
    if tokio::time::timeout(LOG_POST_TIMEOUT, post(line))
        .await
        .is_err()
    {
        tracing::warn!("deployment log sink timed out");
    }
}

fn overflow_line(stream: &str, count: u64) -> String {
    format!(
        "[deployment log buffer overflow: dropped {count} {stream} line(s) while API logging was backpressured]"
    )
}

fn cap_line(line: &str) -> String {
    if line.len() <= LOG_LINE_MAX_BYTES {
        return line.to_string();
    }
    let mut end = LOG_LINE_MAX_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &line[..end])
}

/// Drain one child pipe independently from the API-posting task. This reader
/// never awaits the network; a full queue follows the explicit drop-newest
/// policy in [`LogSender::try_send`].
pub(crate) async fn read_stream_lines<R>(reader: R, sender: LogSender)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = Vec::with_capacity(LOG_LINE_MAX_BYTES);
    loop {
        let Some(complete) = read_bounded_line(&mut reader, &mut line).await else {
            return;
        };
        if complete == 0 && line.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&line);
        sender.try_send(crate::redact(&text));
        if complete == 0 {
            return;
        }
    }
}

/// Read through one newline while retaining at most `LOG_LINE_MAX_BYTES`.
/// Returns `Some(1)` for a newline-terminated line, `Some(0)` for a final
/// unterminated line, and `None` when the stream is empty or has an I/O error.
async fn read_bounded_line<R>(reader: &mut BufReader<R>, line: &mut Vec<u8>) -> Option<usize>
where
    R: AsyncRead + Unpin,
{
    line.clear();
    loop {
        let (consume, copy, newline, eof) = {
            let chunk = reader.fill_buf().await.ok()?;
            if chunk.is_empty() {
                (0, 0, false, true)
            } else if let Some(position) = chunk.iter().position(|byte| *byte == b'\n') {
                let remaining = LOG_LINE_MAX_BYTES.saturating_sub(line.len());
                (position + 1, position.min(remaining), true, false)
            } else {
                let remaining = LOG_LINE_MAX_BYTES.saturating_sub(line.len());
                (chunk.len(), chunk.len().min(remaining), false, false)
            }
        };

        if copy > 0 {
            let chunk = reader.fill_buf().await.ok()?;
            line.extend_from_slice(&chunk[..copy]);
        }
        if consume == 0 {
            return if eof && line.is_empty() {
                None
            } else {
                Some(0)
            };
        }
        reader.consume(consume);
        if newline {
            return Some(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Stdio,
        sync::atomic::{AtomicBool, Ordering},
        time::Instant,
    };
    use tokio::process::Command;

    #[tokio::test]
    async fn readers_drain_pipe_capacity_while_sinks_are_stalled() {
        // Each channel is intentionally never received from: this models an
        // API sink that is stalled while both child pipes keep producing.
        let (stdout_tx, _stdout_rx) = mpsc::channel(LOG_BUFFER_CAPACITY);
        let (stderr_tx, _stderr_rx) = mpsc::channel(LOG_BUFFER_CAPACITY);
        let stdout_dropped = Arc::new(AtomicU64::new(0));
        let stderr_dropped = Arc::new(AtomicU64::new(0));
        let stdout_sender = LogSender::new(stdout_tx, "stdout", stdout_dropped.clone());
        let stderr_sender = LogSender::new(stderr_tx, "stderr", stderr_dropped.clone());
        let mut child = Command::new("sh")
            .args([
                "-c",
                "i=0; while [ $i -lt 20000 ]; do printf 'stdout-%s\\n' \"$i\"; printf 'stderr-%s\\n' \"$i\" >&2; i=$((i+1)); done",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn pipe producer");
        let stdout = child.stdout.take().expect("stdout pipe");
        let stderr = child.stderr.take().expect("stderr pipe");
        let stdout_task = tokio::spawn(read_stream_lines(stdout, stdout_sender.clone()));
        let stderr_task = tokio::spawn(read_stream_lines(stderr, stderr_sender.clone()));

        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("producer must not block on full child pipes")
            .expect("wait for producer");
        assert!(status.success());
        stdout_task.await.expect("stdout reader");
        stderr_task.await.expect("stderr reader");
        assert!(stdout_dropped.load(Ordering::Relaxed) > 0);
        assert!(stderr_dropped.load(Ordering::Relaxed) > 0);
    }

    #[tokio::test]
    async fn shutdown_aborts_a_stalled_sink_within_deadline() {
        let started = Arc::new(AtomicBool::new(false));
        let sink_started = started.clone();
        let post: PostFn = Arc::new(move |_| {
            sink_started.store(true, Ordering::Relaxed);
            Box::pin(std::future::pending())
        });
        let transport = LogTransport::new_with_post("stdout", post);
        let sender = transport.sender();
        sender.try_send("first line".into());
        let started_at = Instant::now();
        while !started.load(Ordering::Relaxed) {
            tokio::task::yield_now().await;
        }
        transport.shutdown().await;
        assert!(started_at.elapsed() < LOG_SHUTDOWN_TIMEOUT + Duration::from_secs(1));
    }

    #[test]
    fn full_queue_reports_dropped_lines() {
        let (tx, mut rx) = mpsc::channel(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let sender = LogSender::new(tx, "stdout", dropped.clone());
        sender.try_send("kept".into());
        sender.try_send("dropped".into());
        assert_eq!(sender.dropped_count(), 1);
        assert_eq!(rx.try_recv().expect("retained line"), "kept");
        sender.try_send("after overflow".into());
        assert_eq!(
            rx.try_recv().expect("overflow marker"),
            overflow_line("stdout", 1)
        );
    }
}
