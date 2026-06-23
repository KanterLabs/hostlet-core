-- Extend per-app storage sampling beyond the managed volume to the whole app
-- disk footprint: the built deployment image and the running container's
-- writable layer, alongside the existing volume `used_bytes`.
--
-- These columns are display-only (the footprint breakdown shown on the Usage
-- screen and the app detail page). The over-quota deploy gate and the per-plan
-- quota stay on `used_bytes` (managed volume only): image size is a function of
-- the build, not user data, so gating deploys on it would block normal apps.
ALTER TABLE app_storage_usage
  ADD COLUMN IF NOT EXISTS image_bytes BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS container_bytes BIGINT NOT NULL DEFAULT 0;
