CREATE TABLE IF NOT EXISTS app_health_snapshots (
  app_id UUID PRIMARY KEY REFERENCES apps(id) ON DELETE CASCADE,
  deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  container_name TEXT,
  status TEXT NOT NULL DEFAULT 'unknown',
  checked_url TEXT,
  http_status INTEGER,
  latency_ms INTEGER,
  failure_count INTEGER NOT NULL DEFAULT 0,
  success_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  last_checked_at TIMESTAMPTZ,
  last_healthy_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS app_health_events (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  deployment_id UUID REFERENCES deployments(id) ON DELETE SET NULL,
  container_name TEXT,
  status TEXT NOT NULL,
  checked_url TEXT,
  http_status INTEGER,
  latency_ms INTEGER,
  error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_app_health_events_app_created
  ON app_health_events(app_id, created_at DESC);
