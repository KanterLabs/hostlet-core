ALTER TABLE agent_jobs
  ADD COLUMN IF NOT EXISTS deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS payload_json JSONB,
  ADD COLUMN IF NOT EXISTS result_json JSONB,
  ADD COLUMN IF NOT EXISTS priority INTEGER NOT NULL DEFAULT 100,
  ADD COLUMN IF NOT EXISTS attempt INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS max_attempts INTEGER NOT NULL DEFAULT 3,
  ADD COLUMN IF NOT EXISTS claimed_by TEXT,
  ADD COLUMN IF NOT EXISTS claimed_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS started_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS last_error TEXT;

UPDATE agent_jobs
SET payload_json = '{}'::jsonb
WHERE payload_json IS NULL;

CREATE INDEX IF NOT EXISTS idx_agent_jobs_claim
  ON agent_jobs(server_id, status, priority, created_at)
  WHERE status IN ('queued','claimed','running');

CREATE INDEX IF NOT EXISTS idx_agent_jobs_deployment_created
  ON agent_jobs(deployment_id, created_at DESC);
