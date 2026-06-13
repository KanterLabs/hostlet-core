use crate::state::AppState;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// Files younger than this threshold are skipped by the orphan sweep.
/// store_uploaded_screenshot renames tmp→final BEFORE the DB INSERT commits,
/// so a young final file may have no row yet, and a young .tmp may be an
/// in-flight upload.
const SWEEP_AGE_THRESHOLD: Duration = Duration::from_secs(3600);

/// Writes `body` to `final_path` via a `tmp_path` rename. On any failure
/// (create/write/flush/rename), best-effort removes `tmp_path` before
/// returning the error — exactly one cleanup point for .tmp files.
pub(in crate::screenshots) async fn write_screenshot_file(
    tmp_path: &Path,
    final_path: &Path,
    body: &[u8],
) -> std::io::Result<()> {
    let result: std::io::Result<()> = async {
        let mut file = tokio::fs::File::create(tmp_path).await?;
        file.write_all(body).await?;
        file.flush().await?;
        tokio::fs::rename(tmp_path, final_path).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(tmp_path).await;
    }
    result
}

/// Returns `true` when the file is a sweep candidate: UUID-parseable stem,
/// known extension (jpg/webp/tmp), and age at or above `SWEEP_AGE_THRESHOLD`.
/// Factored as a pure function so it is unit-testable without a database.
fn is_sweep_candidate(stem: &str, extension: &str, age: Duration) -> bool {
    matches!(extension, "jpg" | "webp" | "tmp")
        && Uuid::parse_str(stem).is_ok()
        && age >= SWEEP_AGE_THRESHOLD
}

/// Removes screenshot files that have no corresponding `app_screenshots` row.
///
/// Reads `state.screenshot_dir` once at startup. Files with mtime within the
/// last hour are skipped (race guard: a file just written may not have its DB
/// row committed yet). Old `.tmp` files are removed unconditionally (stale
/// upload reaping — implemented here, NOT per-upload, to keep read_dir off
/// the hot upload path). Old `.jpg`/`.webp` files are removed only when no
/// row references them; DB errors cause the file to be skipped rather than
/// deleted. Per-file failures log-and-continue. One info summary is logged
/// when any files are removed.
pub async fn sweep_orphaned_screenshot_files(state: &AppState) {
    let mut entries = match tokio::fs::read_dir(&state.screenshot_dir).await {
        Ok(rd) => rd,
        Err(err) => {
            tracing::debug!(error = %err, "screenshot sweep: directory unreadable, skipping");
            return;
        }
    };

    let now = std::time::SystemTime::now();
    let mut removed: u32 = 0;

    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "screenshot sweep: failed to read directory entry");
                continue;
            }
        };

        let path = entry.path();
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(error = %err, path = ?path, "screenshot sweep: failed to read metadata");
                continue;
            }
        };
        if !meta.is_file() {
            continue;
        }

        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let age = meta
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or(Duration::ZERO);

        if !is_sweep_candidate(stem, ext, age) {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if ext == "tmp" {
            // Old .tmp = stale upload; remove unconditionally.
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(file = %file_name, "screenshot sweep: removed stale .tmp");
                    removed += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        file = %file_name,
                        "screenshot sweep: failed to remove stale .tmp"
                    );
                }
            }
            continue;
        }

        // For jpg/webp: only remove when no DB row references the file.
        let row_exists: Result<bool, _> = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM app_screenshots WHERE storage_path=$1)",
        )
        .bind(&file_name)
        .fetch_one(&state.db)
        .await;

        match row_exists {
            Err(err) => {
                // Never delete on uncertainty; skip and let the next sweep retry.
                tracing::warn!(
                    error = %err,
                    file = %file_name,
                    "screenshot sweep: DB error, skipping file"
                );
            }
            Ok(true) => {}
            Ok(false) => match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(
                        file = %file_name,
                        "screenshot sweep: removed orphaned screenshot"
                    );
                    removed += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        file = %file_name,
                        "screenshot sweep: failed to remove orphan"
                    );
                }
            },
        }
    }

    if removed > 0 {
        tracing::info!(
            removed,
            "screenshot sweep: removed orphaned screenshot files"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: Duration = Duration::from_secs(7200);
    const YOUNG: Duration = Duration::from_secs(1800);
    const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn sweep_candidate_old_jpg_accepted() {
        assert!(is_sweep_candidate(UUID, "jpg", OLD));
    }

    #[test]
    fn sweep_candidate_old_webp_accepted() {
        assert!(is_sweep_candidate(UUID, "webp", OLD));
    }

    #[test]
    fn sweep_candidate_old_tmp_accepted() {
        assert!(is_sweep_candidate(UUID, "tmp", OLD));
    }

    #[test]
    fn sweep_candidate_young_file_rejected() {
        assert!(!is_sweep_candidate(UUID, "jpg", YOUNG));
        assert!(!is_sweep_candidate(UUID, "tmp", Duration::ZERO));
    }

    #[test]
    fn sweep_candidate_non_uuid_stem_rejected() {
        assert!(!is_sweep_candidate("screenshot", "jpg", OLD));
        assert!(!is_sweep_candidate("not-a-uuid", "webp", OLD));
    }

    #[test]
    fn sweep_candidate_unknown_extension_rejected() {
        assert!(!is_sweep_candidate(UUID, "png", OLD));
        assert!(!is_sweep_candidate(UUID, "log", OLD));
        assert!(!is_sweep_candidate(UUID, "", OLD));
    }

    #[tokio::test]
    async fn write_screenshot_file_cleans_up_tmp_on_rename_failure() {
        let dir = std::env::temp_dir().join(format!("hostlet-ss-write-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let tmp_path = dir.join("test.tmp");
        // A directory at final_path causes rename to fail.
        let final_path = dir.join("test.jpg");
        tokio::fs::create_dir_all(&final_path).await.unwrap();
        let result = write_screenshot_file(&tmp_path, &final_path, b"test-data").await;
        assert!(result.is_err(), "rename into a directory must fail");
        assert!(
            !tmp_path.exists(),
            ".tmp must be cleaned up after rename failure"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
