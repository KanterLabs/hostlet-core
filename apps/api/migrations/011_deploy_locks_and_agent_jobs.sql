UPDATE deployments
SET status = 'failed',
    failure_summary = COALESCE(
      failure_summary,
      'Deployment was interrupted before completion. Start a new deployment to retry.'
    ),
    finished_at = now()
WHERE status IN ('queued','running','building','starting','health_checking','routing')
  AND COALESCE(started_at, created_at) < now() - interval '30 minutes';

WITH ranked AS (
  SELECT
    id,
    row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC, id DESC) AS rn
  FROM deployments
  WHERE status IN ('queued','running','building','starting','health_checking','routing')
)
UPDATE deployments
SET status = 'failed',
    failure_summary = COALESCE(
      failure_summary,
      'Deployment was superseded while installing deployment locking.'
    ),
    finished_at = now()
WHERE id IN (SELECT id FROM ranked WHERE rn > 1);

CREATE UNIQUE INDEX IF NOT EXISTS idx_deployments_one_active_per_app
  ON deployments(app_id)
  WHERE status IN (
    'queued',
    'running',
    'building',
    'starting',
    'health_checking',
    'routing'
  );

CREATE TABLE IF NOT EXISTS agent_jobs (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  server_id UUID NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  job_type TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued',
  failure_summary TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_agent_jobs_server_created
  ON agent_jobs(server_id, created_at DESC);
