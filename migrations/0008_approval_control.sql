ALTER TABLE job_controls
    DROP CONSTRAINT IF EXISTS job_controls_action_check,
    ADD CONSTRAINT job_controls_action_check
        CHECK (
            decision <> 'accepted'
            OR action IN ('approve-plan', 'skip-current-item', 'cancel-run')
        );
