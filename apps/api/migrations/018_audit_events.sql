CREATE TABLE IF NOT EXISTS audit_events (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  actor_type TEXT NOT NULL,
  actor_id TEXT,
  event_type TEXT NOT NULL,
  app_id UUID REFERENCES apps(id) ON DELETE SET NULL,
  deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  job_id UUID REFERENCES agent_jobs(id) ON DELETE SET NULL,
  ip_address TEXT,
  user_agent TEXT,
  metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_audit_events_created
  ON audit_events(created_at DESC);

CREATE INDEX IF NOT EXISTS idx_audit_events_app_created
  ON audit_events(app_id, created_at DESC);
