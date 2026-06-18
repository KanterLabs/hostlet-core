use super::*;

/// Asserts a `storage_stats` event is upserted into `app_storage_usage` and that
/// an over-quota app is refused a new deploy before any deployment row is made.
/// Uses a fresh app (no active deployment) so the storage gate — not the
/// active-deployment guard — is what blocks the deploy. Lives in its own
/// submodule to keep the test `mod.rs` under the line cap.
pub(super) async fn assert_over_quota_gate_blocks_deploy(state: &AppState, user_id: Uuid) {
    let app_id = insert_app(state, user_id).await;
    // The latest per-app usage sample is upserted into app_storage_usage.
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "storage_stats",
            "appId": app_id,
            // 6 GB — over the 5 GB self-hosted default limit.
            "usedBytes": 6_000_000_000_i64,
            "volumes": [{ "name": "data", "usedBytes": 6_000_000_000_i64 }],
        }),
    )
    .await;
    let row = sqlx::query("SELECT used_bytes, volumes FROM app_storage_usage WHERE app_id=$1")
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("used_bytes"), 6_000_000_000);
    let volumes: serde_json::Value = row.get("volumes");
    assert_eq!(volumes[0]["name"], "data");

    // Over the limit, a new deploy is refused before any deployment row is made.
    let err = crate::deploy::create_and_send_deploy(state, user_id, app_id, "HEAD")
        .await
        .expect_err("deploy should be blocked when over the storage limit");
    assert!(
        err.to_string().contains("storage limit"),
        "unexpected error: {err}"
    );
}
