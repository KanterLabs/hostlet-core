-- Keep webhook claiming short-lived. The handler must not hold a PostgreSQL
-- connection while it creates deployment work on another pool connection.
ALTER TABLE webhook_events
  ADD COLUMN IF NOT EXISTS processing BOOLEAN NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS processing_started_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS processing_token UUID;

CREATE INDEX IF NOT EXISTS idx_webhook_events_processing_lease
  ON webhook_events(processed, processing, processing_started_at)
  WHERE processed = false;

-- A delivery may fan out to an app only once. Remove duplicate legacy rows
-- before enforcing the invariant; keep the most recently recorded outcome.
DELETE FROM webhook_app_events older
USING webhook_app_events newer
WHERE older.webhook_event_id = newer.webhook_event_id
  AND older.app_id = newer.app_id
  AND (older.created_at, older.id) < (newer.created_at, newer.id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_webhook_app_events_event_app
  ON webhook_app_events(webhook_event_id, app_id);
