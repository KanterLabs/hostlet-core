-- Protocol-v2 deployment execution.  All changes are additive so an upgrade
-- never disturbs the deployment currently serving an app.
ALTER TABLE apps
  ADD COLUMN IF NOT EXISTS pending_deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS route_generation BIGINT NOT NULL DEFAULT 0;

ALTER TABLE deployments
  ADD COLUMN IF NOT EXISTS expected_current_deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS activation_generation BIGINT,
  ADD COLUMN IF NOT EXISTS last_heartbeat_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS recovery_count INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS failure_code TEXT;

ALTER TABLE agent_jobs
  ADD COLUMN IF NOT EXISTS protocol_version INTEGER NOT NULL DEFAULT 1,
  ADD COLUMN IF NOT EXISTS claim_token UUID,
  ADD COLUMN IF NOT EXISTS cancel_requested_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS available_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE INDEX IF NOT EXISTS idx_agent_jobs_available_claim
  ON agent_jobs(server_id, status, available_at, priority, created_at)
  WHERE status = 'queued';

CREATE UNIQUE INDEX IF NOT EXISTS idx_apps_one_pending_activation
  ON apps(pending_deployment_id)
  WHERE pending_deployment_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS app_compose_runtime (
  app_id UUID PRIMARY KEY REFERENCES apps(id) ON DELETE CASCADE,
  stable_project TEXT NOT NULL,
  stable_network TEXT NOT NULL,
  backing_spec_hash TEXT,
  backing_status TEXT NOT NULL DEFAULT 'uninitialized',
  last_applied_deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT app_compose_runtime_backing_status
    CHECK (backing_status IN ('uninitialized','ready','maintenance_required','updating','failed'))
);
