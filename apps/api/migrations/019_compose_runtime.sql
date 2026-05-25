ALTER TABLE apps
  ADD COLUMN IF NOT EXISTS runtime_kind TEXT NOT NULL DEFAULT 'single',
  ADD COLUMN IF NOT EXISTS hostlet_config_path TEXT NOT NULL DEFAULT 'hostlet.yml';

ALTER TABLE deployments
  ADD COLUMN IF NOT EXISTS runtime_kind TEXT NOT NULL DEFAULT 'single',
  ADD COLUMN IF NOT EXISTS compose_project TEXT,
  ADD COLUMN IF NOT EXISTS runtime_metadata JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE IF NOT EXISTS deployment_services (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  deployment_id UUID NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
  app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  service_name TEXT NOT NULL,
  role TEXT NOT NULL,
  container_name TEXT,
  image_tag TEXT,
  target_port INTEGER,
  published_port INTEGER,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(deployment_id, service_name)
);

CREATE INDEX IF NOT EXISTS idx_deployment_services_app
  ON deployment_services(app_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_deployment_services_container
  ON deployment_services(container_name)
  WHERE container_name IS NOT NULL;
