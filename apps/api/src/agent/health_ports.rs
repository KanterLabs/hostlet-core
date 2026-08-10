use super::*;

/// Persist the Docker-observed ports reported by the current route owner.
/// Every write is fenced to the authenticated server, current deployment, and
/// canonical frontend container. A backend update additionally has to match
/// the service identified as `backend` by that deployment's inference receipt.
pub(super) async fn persist_observed_ports(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
    deployment_id: Uuid,
    frontend_container: Option<&str>,
    frontend_port: i32,
    backend: Option<(&str, i32)>,
) {
    // The null branch retains old-agent compatibility only for legacy
    // deployments whose canonical container name is also null.
    let _ = sqlx::query(
        r#"
        UPDATE deployments d
        SET published_port=$1
        FROM apps a
        WHERE d.id=$2
          AND d.server_id=$3
          AND d.app_id=$4
          AND a.id=d.app_id
          AND a.server_id=$3
          AND a.current_deployment_id=d.id
          AND d.status IN ('success','rolled_back')
          AND (
            ($5::text IS NOT NULL AND d.container_name=$5)
            OR ($5::text IS NULL AND d.container_name IS NULL)
          )
        "#,
    )
    .bind(frontend_port)
    .bind(deployment_id)
    .bind(server_id)
    .bind(app_id)
    .bind(frontend_container)
    .execute(&state.db)
    .await;

    // Repeat the exact deployment/app/server fence for the service facts.
    // A valid-looking managed container name alone can never redirect the
    // backend update to a sibling service.
    let _ = sqlx::query(
        r#"
        UPDATE deployment_services ds
        SET published_port = CASE
            WHEN ds.container_name=$5 THEN $6
            WHEN $7::text IS NOT NULL AND ds.container_name=$7 THEN $8
            ELSE ds.published_port
        END
        FROM deployments d
        JOIN apps a ON a.id=d.app_id
        WHERE ds.deployment_id=d.id
          AND ds.app_id=a.id
          AND d.id=$2
          AND d.server_id=$3
          AND d.app_id=$4
          AND a.server_id=$3
          AND a.current_deployment_id=d.id
          AND d.status IN ('success','rolled_back')
          AND $5::text IS NOT NULL
          AND d.container_name=$5
          AND ds.role='web'
          AND (
            ds.container_name=$5
            OR (
              $7::text IS NOT NULL
              AND ds.container_name=$7
              AND EXISTS (
                SELECT 1
                FROM jsonb_array_elements(
                  CASE
                    WHEN jsonb_typeof(d.runtime_metadata #> '{inferenceReceipt,services}') = 'array'
                    THEN d.runtime_metadata #> '{inferenceReceipt,services}'
                    ELSE '[]'::jsonb
                  END
                ) AS inferred_service
                WHERE inferred_service->>'role'='backend'
                  AND inferred_service->>'name'=ds.service_name
              )
            )
          )
        "#,
    )
    .bind(frontend_port)
    .bind(deployment_id)
    .bind(server_id)
    .bind(app_id)
    .bind(frontend_container)
    .bind(frontend_port)
    .bind(backend.as_ref().map(|(container, _)| *container))
    .bind(backend.as_ref().map(|(_, port)| *port))
    .execute(&state.db)
    .await;
}
