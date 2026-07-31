-- Additive runtime-capacity and suspension primitives. Cloud overlays consume
-- these tables through the Core submodule, so this migration deliberately
-- contains no hosted-service policy.

CREATE TABLE IF NOT EXISTS app_suspensions (
  app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  reason TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_by TEXT,
  metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (app_id, reason),
  CONSTRAINT app_suspensions_reason_not_blank CHECK (btrim(reason) <> '')
);

INSERT INTO app_suspensions (app_id, reason, created_by)
SELECT id, 'legacy', 'migration'
FROM apps
WHERE suspended_at IS NOT NULL
ON CONFLICT (app_id, reason) DO NOTHING;

ALTER TABLE servers
  ADD COLUMN IF NOT EXISTS resource_snapshot_json JSONB,
  ADD COLUMN IF NOT EXISTS resource_snapshot_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS max_runtime_memory_mb INTEGER,
  ADD COLUMN IF NOT EXISTS max_running_services INTEGER,
  ADD COLUMN IF NOT EXISTS min_available_memory_mb INTEGER,
  ADD COLUMN IF NOT EXISTS min_disk_free_mb INTEGER;

ALTER TABLE servers
  DROP CONSTRAINT IF EXISTS servers_max_runtime_memory_mb_check,
  ADD CONSTRAINT servers_max_runtime_memory_mb_check
    CHECK (max_runtime_memory_mb IS NULL OR max_runtime_memory_mb > 0),
  DROP CONSTRAINT IF EXISTS servers_max_running_services_check,
  ADD CONSTRAINT servers_max_running_services_check
    CHECK (max_running_services IS NULL OR max_running_services > 0),
  DROP CONSTRAINT IF EXISTS servers_min_available_memory_mb_check,
  ADD CONSTRAINT servers_min_available_memory_mb_check
    CHECK (min_available_memory_mb IS NULL OR min_available_memory_mb >= 0),
  DROP CONSTRAINT IF EXISTS servers_min_disk_free_mb_check,
  ADD CONSTRAINT servers_min_disk_free_mb_check
    CHECK (min_disk_free_mb IS NULL OR min_disk_free_mb >= 0);

CREATE INDEX IF NOT EXISTS idx_app_suspensions_reason
  ON app_suspensions(reason, created_at);

CREATE INDEX IF NOT EXISTS idx_agent_jobs_capacity_wait
  ON agent_jobs(server_id, created_at)
  WHERE status='queued' AND payload_json->>'capacity_wait'='true';
