ALTER TABLE servers
  ADD COLUMN IF NOT EXISTS job_signing_secret_ciphertext TEXT;
