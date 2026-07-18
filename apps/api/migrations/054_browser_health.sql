CREATE TABLE IF NOT EXISTS app_browser_health (
  app_id UUID PRIMARY KEY REFERENCES apps(id) ON DELETE CASCADE,
  deployment_id UUID REFERENCES deployments(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'pending',
  failure TEXT,
  checked_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_app_browser_health_deployment
  ON app_browser_health(deployment_id);
