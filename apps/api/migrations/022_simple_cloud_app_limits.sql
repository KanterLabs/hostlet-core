INSERT INTO cloud_plan_entitlements
  (plan_code, app_limit, memory_limit_mb, cpu_limit, monthly_egress_gb, monthly_build_minutes)
VALUES
  ('student', 1, 512, 0.5, 25, 150),
  ('starter', 2, 512, 0.5, 25, 150),
  ('pro', 4, 512, 0.5, 100, 500)
ON CONFLICT (plan_code) DO UPDATE SET
  app_limit=EXCLUDED.app_limit,
  memory_limit_mb=EXCLUDED.memory_limit_mb,
  cpu_limit=EXCLUDED.cpu_limit,
  monthly_egress_gb=EXCLUDED.monthly_egress_gb,
  monthly_build_minutes=EXCLUDED.monthly_build_minutes,
  updated_at=now();
