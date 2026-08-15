//! Shared repository inventory policy used by API preview and agent deploy.
//!
//! Both callers discover files through different providers (GitHub's recursive
//! tree versus a checked-out filesystem), but they must make the same bounded,
//! deterministic choices before topology inference reads any contents.

use std::cmp::Ordering;

/// Bounds shared by API's remote repository inventory and the agent's
/// immutable-checkout inventory.
pub const REPOSITORY_INVENTORY_MAX_ENTRIES: usize = 10_000;
pub const REPOSITORY_INVENTORY_MAX_RELEVANT_FILES: usize = 1_024;
pub const REPOSITORY_INVENTORY_MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;
pub const REPOSITORY_INVENTORY_MAX_FILE_BYTES: usize = 128 * 1024;

/// Returns whether a provider tree contains no truncation marker. The API
/// cannot safely preview a partial tree because the agent later inventories
/// the immutable checkout in full.
pub fn repository_inventory_response_is_complete(provider_truncated: bool) -> bool {
    !provider_truncated
}

/// Shared entry-count boundary. The exact ceiling is accepted; the next entry
/// is rejected.
pub fn repository_inventory_entry_count_within_bound(count: usize) -> bool {
    count <= REPOSITORY_INVENTORY_MAX_ENTRIES
}

/// Shared relevant-file boundary. The exact ceiling is accepted; a candidate
/// beyond it is rejected rather than silently dropped.
pub fn repository_inventory_relevant_file_count_within_bound(count: usize) -> bool {
    count <= REPOSITORY_INVENTORY_MAX_RELEVANT_FILES
}

/// Returns whether a file can contribute contents to the bounded inventory at
/// the current cumulative content size.
pub fn repository_inventory_file_content_within_bound(
    content_bytes: usize,
    file_bytes: usize,
) -> bool {
    file_bytes <= REPOSITORY_INVENTORY_MAX_FILE_BYTES
        && content_bytes
            .checked_add(file_bytes)
            .is_some_and(|total| total <= REPOSITORY_INVENTORY_MAX_CONTENT_BYTES)
}

/// Directory names skipped by both recursive inventory builders. The directory
/// entry itself still counts toward the provider/checkout entry ceiling, while
/// descendants are not traversed or considered relevant.
pub fn repository_inventory_noise_directory(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules"
                | "dist"
                | "build"
                | "out"
                | "target"
                | "vendor"
                | "coverage"
                | "docs"
                | "test"
                | "tests"
                | "fixtures"
                | "examples"
        )
}

/// Returns whether an entry's parent directories are traversable. A noise
/// directory itself is visible and counted; entries below it are not.
pub fn repository_inventory_entry_is_visible(path: &str) -> bool {
    let mut components = path.split('/').peekable();
    while components.peek().is_some() {
        let component = components.next().unwrap_or_default();
        if components.peek().is_some() && repository_inventory_noise_directory(component) {
            return false;
        }
    }
    true
}

/// Returns whether a visible file can affect topology inference.
pub fn repository_inventory_path(path: &str) -> bool {
    // The API uses accepted paths in GitHub's `/contents/{path}` URL. Reject
    // query/fragment delimiters at the shared boundary so the API never
    // interprets a literal checkout filename differently from the agent.
    if path.chars().any(|character| matches!(character, '?' | '#')) {
        return false;
    }
    let filename = path.rsplit('/').next().unwrap_or(path);
    matches!(
        filename,
        "package.json"
            | "pnpm-workspace.yaml"
            | "pnpm-lock.yaml"
            | "package-lock.json"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "go.work"
            | "Cargo.toml"
            | "index.html"
            | "main.rs"
    ) || path.ends_with(".go")
        || matches!(
            path.rsplit('.').next(),
            Some("js" | "jsx" | "ts" | "tsx" | "vue" | "svelte")
        )
}

/// Lockfiles are included by path so package-manager detection can inspect
/// their presence without spending the content budget on lockfile text.
pub fn repository_inventory_lock_file(path: &str) -> bool {
    matches!(
        path.rsplit('/').next(),
        Some("pnpm-lock.yaml" | "package-lock.json" | "yarn.lock" | "bun.lock" | "bun.lockb")
    )
}

/// Metadata collected by either inventory provider before bounded selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryInventoryCandidate {
    pub path: String,
    pub file_bytes: usize,
    pub is_lock_file: bool,
}

impl RepositoryInventoryCandidate {
    pub fn new(path: impl Into<String>, file_bytes: usize) -> Self {
        let path = path.into();
        let is_lock_file = repository_inventory_lock_file(&path);
        Self {
            path,
            file_bytes,
            is_lock_file,
        }
    }
}

/// A deterministic decision shared by API downloads and agent reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryInventorySelection {
    pub path: String,
    pub include_contents: bool,
}

/// Sorts candidates by path, enforces the relevant-file bound, and applies the
/// per-file/cumulative content ceilings in that same order for both callers.
pub fn repository_inventory_select_candidates(
    mut candidates: Vec<RepositoryInventoryCandidate>,
) -> Result<Vec<RepositoryInventorySelection>, &'static str> {
    if !repository_inventory_relevant_file_count_within_bound(candidates.len()) {
        return Err("repository has too many topology-relevant files");
    }
    candidates.sort_by(|left, right| match left.path.cmp(&right.path) {
        Ordering::Equal => left.file_bytes.cmp(&right.file_bytes),
        ordering => ordering,
    });
    let mut content_bytes = 0usize;
    Ok(candidates
        .into_iter()
        .map(|candidate| {
            let include_contents = !candidate.is_lock_file
                && repository_inventory_file_content_within_bound(
                    content_bytes,
                    candidate.file_bytes,
                );
            if include_contents {
                content_bytes = content_bytes.saturating_add(candidate.file_bytes);
            }
            RepositoryInventorySelection {
                path: candidate.path,
                include_contents,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_and_relevant_selection_match_checkout_and_tree_policy() {
        assert!(repository_inventory_entry_is_visible("docs"));
        assert!(!repository_inventory_entry_is_visible("docs/package.json"));
        assert!(!repository_inventory_entry_is_visible("src/tests/app.js"));
        assert!(repository_inventory_entry_is_visible("src/app.js"));
        assert!(repository_inventory_path("src/app.js"));
        assert!(!repository_inventory_path("src/app?.js"));
        assert!(!repository_inventory_path("src/app#.js"));
        assert!(!repository_inventory_path("src/README.md"));
        assert!(repository_inventory_lock_file("pnpm-lock.yaml"));
    }

    #[test]
    fn selection_is_sorted_and_applies_content_ceiling_once() {
        let selected = repository_inventory_select_candidates(vec![
            RepositoryInventoryCandidate::new("z.js", REPOSITORY_INVENTORY_MAX_FILE_BYTES),
            RepositoryInventoryCandidate::new("a.js", REPOSITORY_INVENTORY_MAX_CONTENT_BYTES),
            RepositoryInventoryCandidate::new(
                "pnpm-lock.yaml",
                REPOSITORY_INVENTORY_MAX_FILE_BYTES,
            ),
        ])
        .unwrap();
        assert_eq!(
            selected,
            vec![
                RepositoryInventorySelection {
                    path: "a.js".to_string(),
                    include_contents: false,
                },
                RepositoryInventorySelection {
                    path: "pnpm-lock.yaml".to_string(),
                    include_contents: false,
                },
                RepositoryInventorySelection {
                    path: "z.js".to_string(),
                    include_contents: true,
                },
            ]
        );
    }

    #[test]
    fn relevant_candidate_bound_accepts_exactly_max_and_rejects_next() {
        let exact = (0..REPOSITORY_INVENTORY_MAX_RELEVANT_FILES)
            .map(|index| RepositoryInventoryCandidate::new(format!("{index}.js"), 1))
            .collect();
        assert_eq!(
            repository_inventory_select_candidates(exact).unwrap().len(),
            REPOSITORY_INVENTORY_MAX_RELEVANT_FILES
        );

        let over = (0..=REPOSITORY_INVENTORY_MAX_RELEVANT_FILES)
            .map(|index| RepositoryInventoryCandidate::new(format!("{index}.js"), 1))
            .collect();
        assert!(repository_inventory_select_candidates(over).is_err());
    }
}
