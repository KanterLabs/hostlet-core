use super::*;

pub(in crate::agent) async fn handle_socket(state: AppState, server_id: Uuid, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(32);
    // Each socket tags itself with a fresh connection_id so teardown can tell
    // whether it still owns the registry slot (see disconnect_agent).
    let connection_id = Uuid::new_v4();

    if !register_agent(&state, server_id, connection_id, tx).await {
        tracing::warn!(%server_id, "rejected duplicate agent websocket connection");
        let _ = sender.send(Message::Close(None)).await;
        return;
    }

    let db = state.db.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(Message::Text(msg.to_string())).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                handle_agent_message(&state, server_id, value).await;
            }
        }
    }
    send_task.abort();
    disconnect_agent(&state, &db, server_id, connection_id).await;
}

/// Register this connection as the agent for `server_id`, marking the server
/// online. Returns `false` (without registering) if a live connection already
/// owns the slot, so the caller can reject the duplicate.
async fn register_agent(
    state: &AppState,
    server_id: Uuid,
    connection_id: Uuid,
    tx: mpsc::Sender<serde_json::Value>,
) -> bool {
    {
        let mut agents = state.agents.write().await;
        if agents
            .get(&server_id)
            .is_some_and(|connection| !connection.sender.is_closed())
        {
            return false;
        }
        agents.insert(
            server_id,
            AgentConnection {
                connection_id,
                sender: tx,
            },
        );
    }
    let _ = sqlx::query("UPDATE servers SET status='online', last_seen_at=now() WHERE id=$1")
        .bind(server_id)
        .execute(&state.db)
        .await;
    true
}

/// Tear down this connection's registration. We only remove the slot and mark
/// the server offline if *this* connection still owns it: a newer connection
/// may have replaced us in the registry, and it must keep the server online.
async fn disconnect_agent(
    state: &AppState,
    db: &sqlx::PgPool,
    server_id: Uuid,
    connection_id: Uuid,
) {
    let mut agents = state.agents.write().await;
    if agents
        .get(&server_id)
        .is_some_and(|connection| connection_is_current(connection, connection_id))
    {
        agents.remove(&server_id);
        drop(agents);
        let _ = sqlx::query("UPDATE servers SET status='offline' WHERE id=$1")
            .bind(server_id)
            .execute(db)
            .await;
    }
}
