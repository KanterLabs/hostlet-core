CREATE TABLE IF NOT EXISTS app_resource_snapshots (
  container_name TEXT PRIMARY KEY,
  cpu_percent TEXT NOT NULL,
  memory_usage TEXT NOT NULL,
  memory_percent TEXT NOT NULL,
  network_io TEXT NOT NULL,
  block_io TEXT NOT NULL,
  pids TEXT NOT NULL,
  sampled_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
