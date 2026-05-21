DELETE FROM github_accounts a
USING github_accounts b
WHERE a.user_id = b.user_id
  AND a.github_id = b.github_id
  AND (
    a.updated_at < b.updated_at
    OR (a.updated_at = b.updated_at AND a.id::text < b.id::text)
  );

CREATE UNIQUE INDEX IF NOT EXISTS idx_github_accounts_user_github
ON github_accounts(user_id, github_id);

ALTER TABLE deployments
ADD COLUMN IF NOT EXISTS published_port INTEGER;
