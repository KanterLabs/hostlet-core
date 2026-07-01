-- Generic "pause this app" flag. When set, the app's container must be
-- stopped and the app must be excluded from health-target polling (see
-- apps/api/src/agent/routes.rs::health_targets), so the agent's own
-- crash-recovery auto-start loop (apps/agent/src/ops.rs::auto_start_container)
-- cannot bring a deliberately-stopped container back up. Self-hosted has no
-- caller of this column yet; Hostlet Cloud's billing reaper
-- (apps/api/src/web/billing/reaper.rs, cloud repo) sets/clears it when a
-- Stripe subscription goes inactive/active again.
ALTER TABLE apps
  ADD COLUMN IF NOT EXISTS suspended_at TIMESTAMPTZ;
