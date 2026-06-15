ALTER TABLE servers
  ADD COLUMN IF NOT EXISTS capabilities TEXT[] NOT NULL DEFAULT ARRAY['builder','app_runner']::TEXT[],
  ADD COLUMN IF NOT EXISTS draining BOOLEAN NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS max_concurrent_apps INTEGER NOT NULL DEFAULT 8,
  ADD COLUMN IF NOT EXISTS max_concurrent_builds INTEGER NOT NULL DEFAULT 1;

UPDATE servers
SET capabilities = ARRAY['builder','app_runner']::TEXT[]
WHERE capabilities IS NULL OR cardinality(capabilities) = 0;

UPDATE servers
SET max_concurrent_apps = 8
WHERE max_concurrent_apps < 1;

UPDATE servers
SET max_concurrent_builds = 1
WHERE max_concurrent_builds < 1;

CREATE INDEX IF NOT EXISTS idx_servers_capabilities
  ON servers USING GIN (capabilities);

CREATE INDEX IF NOT EXISTS idx_servers_runner_capacity
  ON servers (draining, max_concurrent_apps, created_at);
