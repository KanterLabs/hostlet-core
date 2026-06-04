CREATE TABLE IF NOT EXISTS app_screenshots (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  agent_job_id UUID REFERENCES agent_jobs(id) ON DELETE SET NULL,
  source TEXT NOT NULL DEFAULT 'generated',
  content_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  width INTEGER,
  height INTEGER,
  storage_path TEXT NOT NULL,
  capture_url TEXT,
  captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_app_screenshots_app_captured
  ON app_screenshots(app_id, captured_at DESC);

CREATE INDEX IF NOT EXISTS idx_app_screenshots_deployment
  ON app_screenshots(deployment_id, captured_at DESC);

