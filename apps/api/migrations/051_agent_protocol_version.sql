-- Highest deployment protocol most recently advertised by each agent. This
-- lets the API reject generated-topology work before it creates an unclaimable
-- protocol-v3 job while legacy v1/v2 jobs remain compatible.
ALTER TABLE servers
  ADD COLUMN IF NOT EXISTS agent_protocol_version INTEGER NOT NULL DEFAULT 1;
