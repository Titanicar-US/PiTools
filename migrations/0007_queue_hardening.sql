ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS max_attempts INTEGER NOT NULL DEFAULT 5;

UPDATE jobs
SET attempt = GREATEST(attempt, 0),
    max_attempts = GREATEST(max_attempts, attempt, 1);

UPDATE jobs
SET state = 'cancelled',
    lease_owner = NULL,
    lease_until = NULL,
    lease_token = NULL,
    updated_at = NOW()
WHERE cancel_requested = TRUE
  AND state IN ('queued', 'running', 'waiting_approval');

UPDATE jobs
SET lease_owner = NULL,
    lease_until = NULL,
    lease_token = NULL,
    updated_at = NOW()
WHERE state <> 'running'
  AND (lease_owner IS NOT NULL OR lease_until IS NOT NULL OR lease_token IS NOT NULL);

WITH ranked_active_jobs AS (
    SELECT
        id,
        FIRST_VALUE(id) OVER (
            PARTITION BY repository_id, pull_request_number, kind
            ORDER BY created_at, id
        ) AS canonical_id,
        ROW_NUMBER() OVER (
            PARTITION BY repository_id, pull_request_number, kind
            ORDER BY created_at, id
        ) AS duplicate_rank
    FROM jobs
    WHERE state IN ('queued', 'running', 'waiting_approval')
), duplicate_active_jobs AS (
    SELECT id, canonical_id
    FROM ranked_active_jobs
    WHERE duplicate_rank > 1
)
UPDATE jobs AS job
SET state = 'cancelled',
    cancel_requested = TRUE,
    result = jsonb_build_object(
        'reason', 'deduplicated_by_queue_hardening_migration',
        'canonical_job_id', duplicate.canonical_id::TEXT
    ),
    lease_owner = NULL,
    lease_until = NULL,
    lease_token = NULL,
    updated_at = NOW()
FROM duplicate_active_jobs AS duplicate
WHERE job.id = duplicate.id;

ALTER TABLE jobs
    DROP CONSTRAINT IF EXISTS jobs_attempt_bounds_check,
    ADD CONSTRAINT jobs_attempt_bounds_check
        CHECK (attempt >= 0 AND max_attempts > 0 AND attempt <= max_attempts);

CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_active_logical_identity
    ON jobs (repository_id, pull_request_number, kind)
    WHERE state IN ('queued', 'running', 'waiting_approval');

CREATE INDEX IF NOT EXISTS idx_jobs_leaseable
    ON jobs (created_at, id)
    WHERE cancel_requested = FALSE AND state IN ('queued', 'running');
