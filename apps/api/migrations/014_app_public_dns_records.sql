CREATE TABLE IF NOT EXISTS app_public_dns_records (
  id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
  app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  zone_id TEXT NOT NULL,
  hostname TEXT NOT NULL,
  cloudflare_record_id TEXT NOT NULL,
  target TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(app_id, hostname),
  UNIQUE(zone_id, hostname),
  UNIQUE(zone_id, cloudflare_record_id)
);
