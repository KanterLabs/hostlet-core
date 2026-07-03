use super::*;

/// `queue_priority_offset` must only reorder jobs *within* a job-type band:
/// a base-5 job from a max-offset app still beats every base-20 job, while
/// equal-base jobs are claimed in ascending offset order (then created_at).
#[tokio::test]
async fn db_enqueue_applies_app_queue_priority_offset_within_bands() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let free_app = insert_app(&state, user_id).await;
    let paid_app = insert_app_2(&state, user_id).await;
    set_queue_priority_offset(&state, free_app, 3).await;

    let free_urgent = enqueue(&state, Some(free_app), 5).await;
    let free_normal = enqueue(&state, Some(free_app), 20).await;
    let paid_normal = enqueue(&state, Some(paid_app), 20).await;
    let orphan = enqueue(&state, None, 20).await;

    assert_eq!(job_priority(&state, free_urgent).await, 8);
    assert_eq!(job_priority(&state, free_normal).await, 23);
    assert_eq!(job_priority(&state, paid_normal).await, 20);
    assert_eq!(job_priority(&state, orphan).await, 20);

    let headers = agent_headers(&state, TEST_SERVER_ID);
    assert_eq!(claim_one(&state, &headers).await, free_urgent);
    assert_eq!(claim_one(&state, &headers).await, paid_normal);
    assert_eq!(claim_one(&state, &headers).await, orphan);
    assert_eq!(claim_one(&state, &headers).await, free_normal);
}

async fn set_queue_priority_offset(state: &AppState, app_id: Uuid, offset: i32) {
    sqlx::query("UPDATE apps SET queue_priority_offset=$2 WHERE id=$1")
        .bind(app_id)
        .bind(offset)
        .execute(&state.db)
        .await
        .unwrap();
}

async fn enqueue(state: &AppState, app_id: Option<Uuid>, base_priority: i32) -> Uuid {
    crate::deploy::enqueue_agent_job(
        state,
        TEST_SERVER_ID,
        app_id,
        None,
        "deploy",
        serde_json::json!({"type": "deploy"}),
        base_priority,
    )
    .await
    .unwrap()
}

async fn job_priority(state: &AppState, job_id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT priority FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

/// Claims via the HTTP handler (like the lifecycle tests), returns the claimed
/// job id, and immediately completes it so the next claim sees a clean queue.
async fn claim_one(state: &AppState, headers: &HeaderMap) -> Uuid {
    let response = claim_job(
        State(state.clone()),
        headers.clone(),
        Json(ClaimJobRequest {
            agent_id: Some("ci-agent".into()),
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let id: Uuid = sqlx::query_scalar("SELECT id FROM agent_jobs WHERE status='claimed'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_jobs SET status='success' WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();
    id
}
