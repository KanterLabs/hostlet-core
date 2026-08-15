use super::*;
use hostlet_contracts::{
    repository_inventory_entry_count_within_bound, repository_inventory_entry_is_visible,
    repository_inventory_path, repository_inventory_response_is_complete,
    repository_inventory_select_candidates, RepositoryInventoryCandidate,
    REPOSITORY_INVENTORY_MAX_ENTRIES,
};

/// Fetches the recursive tree once and downloads only small files that can
/// affect topology inference. Lockfiles are represented by path only: manager
/// detection needs their presence, while dependency resolution remains the
/// agent's responsibility and large lock contents never enter API memory.
pub(super) async fn github_repository_inventory(
    state: &AppState,
    repo: &str,
    branch: &str,
    token: Option<&str>,
) -> anyhow::Result<RepositoryInventory> {
    let encoded_branch =
        url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>();
    let mut request = state
        .http
        .get(format!(
            "https://api.github.com/repos/{repo}/git/trees/{encoded_branch}?recursive=1"
        ))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let tree: Value = request.send().await?.error_for_status()?.json().await?;
    if !repository_inventory_response_is_complete(
        tree.get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ) {
        anyhow::bail!("repository inventory is truncated");
    }
    let entries = tree
        .get("tree")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let candidates = github_inventory_candidates(entries)?;
    let selections = repository_inventory_select_candidates(candidates)
        .map_err(|message| anyhow::anyhow!(message))?;

    let mut files = Vec::with_capacity(selections.len());
    for selection in selections {
        let contents = if selection.include_contents {
            github_file_text(state, repo, branch, &selection.path, token).await?
        } else {
            None
        };
        files.push(RepositoryFile {
            path: selection.path,
            contents,
        });
    }
    Ok(RepositoryInventory { files })
}

fn github_inventory_candidates(
    entries: Vec<Value>,
) -> anyhow::Result<Vec<RepositoryInventoryCandidate>> {
    let entries = entries
        .into_iter()
        .filter(|entry| {
            entry
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(repository_inventory_entry_is_visible)
        })
        .collect::<Vec<_>>();
    if !repository_inventory_entry_count_within_bound(entries.len()) {
        anyhow::bail!("repository inventory exceeds {REPOSITORY_INVENTORY_MAX_ENTRIES} entries");
    }
    Ok(entries
        .into_iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("blob"))
        .filter_map(|entry| {
            let path = entry.get("path")?.as_str()?;
            repository_inventory_path(path).then(|| {
                RepositoryInventoryCandidate::new(
                    path,
                    usize::try_from(
                        entry
                            .get("size")
                            .and_then(Value::as_u64)
                            .unwrap_or_default(),
                    )
                    .unwrap_or(usize::MAX),
                )
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_inventory_rejects_url_delimiters_before_contents_download() {
        let candidates = github_inventory_candidates(vec![
            serde_json::json!({"type": "blob", "path": "src/app?.js", "size": 1}),
            serde_json::json!({"type": "blob", "path": "src/app#.js", "size": 1}),
            serde_json::json!({"type": "blob", "path": "src/app.js", "size": 1}),
        ])
        .unwrap();
        assert_eq!(
            candidates,
            vec![RepositoryInventoryCandidate::new("src/app.js", 1)]
        );
    }
}
