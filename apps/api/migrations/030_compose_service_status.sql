-- Per-service status for multi-service (Compose) deployments.
--
-- `deployment_services` was created in 019_compose_runtime.sql but never read or
-- written. These mutable columns let the agent report each service's lifecycle
-- and (for the web service) health, so the UI can render a live card per
-- service. All columns are nullable/additive — safe on existing rows.
ALTER TABLE deployment_services
  ADD COLUMN IF NOT EXISTS status TEXT,
  ADD COLUMN IF NOT EXISTS health_status TEXT,
  ADD COLUMN IF NOT EXISTS last_checked_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS last_healthy_at TIMESTAMPTZ;
