ALTER TABLE feedback_items
    ADD COLUMN IF NOT EXISTS repair_state TEXT NOT NULL DEFAULT 'unreviewed',
    ADD COLUMN IF NOT EXISTS repair_path TEXT,
    ADD COLUMN IF NOT EXISTS repair_patch_hash TEXT,
    ADD COLUMN IF NOT EXISTS repair_validated_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS repair_applied_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS resolution_eligible BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE feedback_items
    DROP CONSTRAINT IF EXISTS feedback_items_repair_state_check,
    ADD CONSTRAINT feedback_items_repair_state_check
        CHECK (repair_state IN ('unreviewed', 'rejected', 'previewed', 'applied'));

ALTER TABLE feedback_items
    DROP CONSTRAINT IF EXISTS feedback_items_resolution_eligibility_check,
    ADD CONSTRAINT feedback_items_resolution_eligibility_check
        CHECK (
            NOT resolution_eligible
            OR (repair_state = 'applied' AND repair_applied_at IS NOT NULL)
        );

CREATE INDEX IF NOT EXISTS idx_feedback_items_repair_state
    ON feedback_items (repository_id, pull_request_number, repair_state)
    WHERE is_automation = TRUE AND resolved = FALSE;
