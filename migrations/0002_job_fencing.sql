ALTER TABLE jobs ADD COLUMN IF NOT EXISTS lease_token UUID;

CREATE INDEX IF NOT EXISTS idx_jobs_cancelled ON jobs (cancel_requested, state);
