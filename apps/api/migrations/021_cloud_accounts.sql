CREATE TABLE IF NOT EXISTS cloud_users (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  github_id BIGINT UNIQUE NOT NULL,
  login TEXT NOT NULL,
  name TEXT,
  email TEXT,
  avatar_url TEXT,
  status TEXT NOT NULL DEFAULT 'active',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cloud_sessions (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  cloud_user_id UUID NOT NULL REFERENCES cloud_users(id) ON DELETE CASCADE,
  token_hash TEXT NOT NULL UNIQUE,
  user_agent TEXT,
  ip_address TEXT,
  expires_at TIMESTAMPTZ NOT NULL,
  revoked_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cloud_github_installations (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  cloud_user_id UUID NOT NULL REFERENCES cloud_users(id) ON DELETE CASCADE,
  installation_id BIGINT NOT NULL UNIQUE,
  account_login TEXT NOT NULL,
  account_type TEXT NOT NULL,
  permissions_json JSONB NOT NULL DEFAULT '{}'::jsonb,
  repository_selection TEXT NOT NULL DEFAULT 'selected',
  suspended_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cloud_stripe_customers (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  cloud_user_id UUID NOT NULL REFERENCES cloud_users(id) ON DELETE CASCADE,
  stripe_customer_id TEXT NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(cloud_user_id)
);

CREATE TABLE IF NOT EXISTS cloud_subscriptions (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  cloud_user_id UUID NOT NULL REFERENCES cloud_users(id) ON DELETE CASCADE,
  stripe_subscription_id TEXT UNIQUE,
  plan_code TEXT NOT NULL,
  status TEXT NOT NULL,
  current_period_start TIMESTAMPTZ,
  current_period_end TIMESTAMPTZ,
  cancel_at_period_end BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cloud_plan_entitlements (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  plan_code TEXT NOT NULL UNIQUE,
  app_limit INTEGER NOT NULL,
  memory_limit_mb INTEGER NOT NULL,
  cpu_limit DOUBLE PRECISION NOT NULL,
  monthly_egress_gb INTEGER NOT NULL,
  monthly_build_minutes INTEGER NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cloud_usage_buckets (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  cloud_user_id UUID NOT NULL REFERENCES cloud_users(id) ON DELETE CASCADE,
  app_id UUID REFERENCES apps(id) ON DELETE CASCADE,
  bucket_start TIMESTAMPTZ NOT NULL,
  build_seconds INTEGER NOT NULL DEFAULT 0,
  runtime_seconds INTEGER NOT NULL DEFAULT 0,
  egress_bytes BIGINT NOT NULL DEFAULT 0,
  peak_memory_mb INTEGER,
  cpu_seconds DOUBLE PRECISION NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(cloud_user_id, app_id, bucket_start)
);

CREATE TABLE IF NOT EXISTS cloud_webhook_events (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  provider TEXT NOT NULL,
  provider_event_id TEXT NOT NULL,
  payload JSONB NOT NULL,
  processed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(provider, provider_event_id)
);

INSERT INTO cloud_plan_entitlements
  (plan_code, app_limit, memory_limit_mb, cpu_limit, monthly_egress_gb, monthly_build_minutes)
VALUES
  ('student', 1, 512, 0.5, 25, 150),
  ('starter', 1, 512, 0.5, 25, 150),
  ('pro', 3, 512, 0.5, 100, 500)
ON CONFLICT (plan_code) DO UPDATE SET
  app_limit=EXCLUDED.app_limit,
  memory_limit_mb=EXCLUDED.memory_limit_mb,
  cpu_limit=EXCLUDED.cpu_limit,
  monthly_egress_gb=EXCLUDED.monthly_egress_gb,
  monthly_build_minutes=EXCLUDED.monthly_build_minutes,
  updated_at=now();

CREATE INDEX IF NOT EXISTS idx_cloud_sessions_user_expires
  ON cloud_sessions(cloud_user_id, expires_at DESC);

CREATE INDEX IF NOT EXISTS idx_cloud_github_installations_user
  ON cloud_github_installations(cloud_user_id);

CREATE INDEX IF NOT EXISTS idx_cloud_subscriptions_user
  ON cloud_subscriptions(cloud_user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_cloud_usage_buckets_user_start
  ON cloud_usage_buckets(cloud_user_id, bucket_start DESC);
