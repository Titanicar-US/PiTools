ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS check_run_url TEXT;

ALTER TABLE job_controls
    ADD COLUMN IF NOT EXISTS request_id TEXT,
    ADD COLUMN IF NOT EXISTS target_item_id TEXT,
    ADD COLUMN IF NOT EXISTS decision TEXT NOT NULL DEFAULT 'accepted';

UPDATE job_controls
SET request_id = id::TEXT
WHERE request_id IS NULL;

ALTER TABLE job_controls
    ALTER COLUMN request_id SET NOT NULL,
    DROP CONSTRAINT IF EXISTS job_controls_job_id_action_actor_login_key,
    ADD CONSTRAINT job_controls_action_check
        CHECK (
            decision <> 'accepted'
            OR action IN ('skip-current-item', 'cancel-run')
        ),
    ADD CONSTRAINT job_controls_decision_check
        CHECK (decision IN ('accepted', 'rejected-unauthorized', 'rejected-duplicate', 'rejected-unknown')),
    ADD CONSTRAINT job_controls_job_request_unique UNIQUE (job_id, request_id);

ALTER TABLE comments
    ADD COLUMN IF NOT EXISTS immutable BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

CREATE UNIQUE INDEX IF NOT EXISTS idx_comments_job_marker
    ON comments (job_id, marker);
