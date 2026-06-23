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
    let row = sqlx::query(
        "SELECT used_bytes, image_bytes, container_bytes, volumes \
         FROM app_storage_usage WHERE app_id=$1",
    )
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("used_bytes"), 6_000_000_000);
    // The message omitted the footprint fields (older agent) — they default to 0.
    assert_eq!(row.get::<i64, _>("image_bytes"), 0);
    assert_eq!(row.get::<i64, _>("container_bytes"), 0);
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

    // A second app with a tiny volume but a huge image + container layer: the
    // footprint fields are stored, and the over-quota gate stays volume-only, so
    // the large image must not block its deploy.
    let big_image_app = insert_app(state, user_id).await;
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "storage_stats",
            "appId": big_image_app,
            "usedBytes": 1_000_i64,
            "imageBytes": 8_000_000_000_i64,
            "containerBytes": 2_000_000_000_i64,
            "volumes": [{ "name": "data", "usedBytes": 1_000_i64 }],
        }),
    )
    .await;
    let footprint =
        sqlx::query("SELECT image_bytes, container_bytes FROM app_storage_usage WHERE app_id=$1")
            .bind(big_image_app)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(footprint.get::<i64, _>("image_bytes"), 8_000_000_000);
    assert_eq!(footprint.get::<i64, _>("container_bytes"), 2_000_000_000);
    if let Err(err) =
        crate::deploy::create_and_send_deploy(state, user_id, big_image_app, "HEAD").await
    {
        assert!(
            !err.to_string().contains("storage limit"),
            "image/container bytes must not trip the volume quota gate: {err}"
        );
    }
}
