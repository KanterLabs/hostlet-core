-- Per-app managed-volume storage usage, sampled by the agent (`docker system
-- df -v`) for the quota meter + the over-quota deploy gate.
--
-- One latest row per app (upserted on the agent's `storage_stats` event):
-- `used_bytes` is the combined size of every managed volume the app owns (its
-- web/data volume plus each add-on volume), and `volumes` is the per-volume
-- breakdown for the per-service UI. The limit itself is not stored here — it
-- comes from `runtime_config.compose.volumeStorageLimitMb` (Cloud-injected per
-- plan) or the self-hosted default.
CREATE TABLE IF NOT EXISTS app_storage_usage (
    app_id      UUID PRIMARY KEY REFERENCES apps(id) ON DELETE CASCADE,
    used_bytes  BIGINT NOT NULL DEFAULT 0,
    volumes     JSONB NOT NULL DEFAULT '[]'::jsonb,
    sampled_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
