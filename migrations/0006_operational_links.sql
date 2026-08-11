ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS check_run_id BIGINT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_check_run_id
    ON jobs (check_run_id)
    WHERE check_run_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_jobs_pr_state
    ON jobs (repository_id, pull_request_number, state, created_at DESC);
