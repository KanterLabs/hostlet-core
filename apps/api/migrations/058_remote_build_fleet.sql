-- Remote build pools and disposable OCI artifact transport.  Existing installs
-- retain a local default pool; remote builders are opt-in and never become app
-- runners merely by enrolling.

CREATE TABLE IF NOT EXISTS build_pools (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  name TEXT NOT NULL,
  provider TEXT NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT true,
  is_default BOOLEAN NOT NULL DEFAULT false,
  max_concurrent_builds INTEGER NOT NULL DEFAULT 1,
  supported_platforms TEXT[] NOT NULL DEFAULT ARRAY['linux/amd64']::TEXT[],
  config_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  config_ciphertext TEXT,
  qualification_status TEXT NOT NULL DEFAULT 'not_required',
  qualification_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT build_pools_provider_check
    CHECK (provider IN ('local','vm','cloudflare')),
  CONSTRAINT build_pools_concurrency_check
    CHECK (max_concurrent_builds > 0),
  CONSTRAINT build_pools_qualification_check
    CHECK (qualification_status IN ('not_required','pending','passed','failed'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_build_pools_one_default
  ON build_pools (is_default)
  WHERE is_default;

INSERT INTO build_pools
  (id,name,provider,enabled,is_default,max_concurrent_builds,supported_platforms)
VALUES
  ('00000000-0000-0000-0000-000000000010','Local builder','local',true,true,1,
   ARRAY['linux/amd64','linux/arm64']::TEXT[])
ON CONFLICT (id) DO NOTHING;

ALTER TABLE servers
  ADD COLUMN IF NOT EXISTS build_pool_id UUID REFERENCES build_pools(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS platforms TEXT[] NOT NULL DEFAULT ARRAY['linux/amd64']::TEXT[],
  ADD COLUMN IF NOT EXISTS last_build_assigned_at TIMESTAMPTZ;

UPDATE servers
SET build_pool_id='00000000-0000-0000-0000-000000000010'
WHERE kind='local' AND build_pool_id IS NULL;

ALTER TABLE apps
  ADD COLUMN IF NOT EXISTS build_pool_id UUID REFERENCES build_pools(id) ON DELETE SET NULL;

CREATE TABLE IF NOT EXISTS builder_enrollment_tokens (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  build_pool_id UUID NOT NULL REFERENCES build_pools(id) ON DELETE CASCADE,
  owner_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash TEXT NOT NULL UNIQUE,
  expires_at TIMESTAMPTZ NOT NULL,
  consumed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS deployment_builds (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  deployment_id UUID NOT NULL UNIQUE REFERENCES deployments(id) ON DELETE CASCADE,
  build_pool_id UUID NOT NULL REFERENCES build_pools(id) ON DELETE RESTRICT,
  status TEXT NOT NULL DEFAULT 'queued',
  required_platform TEXT NOT NULL,
  build_plan_digest TEXT,
  build_spec_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  waiting_reason TEXT,
  artifact_manifest_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  failure_code TEXT,
  failure_summary TEXT,
  started_at TIMESTAMPTZ,
  finished_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT deployment_builds_status_check CHECK (status IN (
    'queued','leased','building','publishing','retry_wait',
    'succeeded','failed','canceled'
  ))
);

ALTER TABLE deployment_builds
  ADD COLUMN IF NOT EXISTS build_spec_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN IF NOT EXISTS waiting_reason TEXT;

ALTER TABLE deployments
  ADD COLUMN IF NOT EXISTS build_id UUID REFERENCES deployment_builds(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS artifact_manifest_json JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE agent_jobs
  ALTER COLUMN server_id DROP NOT NULL,
  ADD COLUMN IF NOT EXISTS build_pool_id UUID REFERENCES build_pools(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_servers_build_pool
  ON servers (build_pool_id,draining,status,last_build_assigned_at)
  WHERE capabilities @> ARRAY['builder']::TEXT[];

CREATE INDEX IF NOT EXISTS idx_deployment_builds_pool_status
  ON deployment_builds (build_pool_id,status,created_at);

CREATE INDEX IF NOT EXISTS idx_agent_jobs_pool_claim
  ON agent_jobs (build_pool_id,status,available_at,priority,created_at)
  WHERE server_id IS NULL AND status='queued';

CREATE INDEX IF NOT EXISTS idx_builder_enrollment_tokens_expiry
  ON builder_enrollment_tokens (expires_at)
  WHERE consumed_at IS NULL;

DROP INDEX IF EXISTS idx_deployments_one_active_per_app;
CREATE UNIQUE INDEX idx_deployments_one_active_per_app
  ON deployments(app_id)
  WHERE status IN (
    'queued','queued_for_build','running','building','publishing',
    'queued_for_release','pulling','starting','health_checking','routing'
  );
