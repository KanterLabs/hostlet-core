ALTER TABLE apps
  ADD COLUMN IF NOT EXISTS packaging_strategy TEXT NOT NULL DEFAULT 'auto';

UPDATE apps
SET packaging_strategy='auto'
WHERE packaging_strategy IS NULL OR packaging_strategy NOT IN ('auto', 'dockerfile', 'generated');

ALTER TABLE apps
  ADD CONSTRAINT apps_packaging_strategy_check
  CHECK (packaging_strategy IN ('auto', 'dockerfile', 'generated'))
  NOT VALID;

ALTER TABLE apps VALIDATE CONSTRAINT apps_packaging_strategy_check;
