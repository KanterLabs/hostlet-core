-- Per-app queue priority offset, added to the base job-type priority at
-- enqueue time (see enqueue_agent_job). Lower is claimed first. The offset
-- must stay smaller than the smallest gap between base job-type priorities
-- (currently 5) so it can only reorder jobs within the same job-type band;
-- the CHECK enforces that ceiling. Self-hosted installs keep the default 0.
ALTER TABLE apps
  ADD COLUMN IF NOT EXISTS queue_priority_offset INTEGER NOT NULL DEFAULT 0
  CHECK (queue_priority_offset BETWEEN 0 AND 4);
