ALTER TABLE agent_jobs
  ADD COLUMN IF NOT EXISTS app_id UUID REFERENCES apps(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_agent_jobs_app_created
  ON agent_jobs(app_id, created_at DESC);
