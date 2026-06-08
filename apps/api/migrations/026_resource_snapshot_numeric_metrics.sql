ALTER TABLE app_resource_snapshots
  ADD COLUMN IF NOT EXISTS cpu_percent_value DOUBLE PRECISION,
  ADD COLUMN IF NOT EXISTS memory_usage_bytes BIGINT,
  ADD COLUMN IF NOT EXISTS memory_limit_bytes BIGINT,
  ADD COLUMN IF NOT EXISTS memory_percent_value DOUBLE PRECISION,
  ADD COLUMN IF NOT EXISTS network_rx_bytes BIGINT,
  ADD COLUMN IF NOT EXISTS network_tx_bytes BIGINT,
  ADD COLUMN IF NOT EXISTS block_read_bytes BIGINT,
  ADD COLUMN IF NOT EXISTS block_write_bytes BIGINT,
  ADD COLUMN IF NOT EXISTS pids_current BIGINT;
