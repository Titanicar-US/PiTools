ALTER TABLE job_controls
    DROP CONSTRAINT IF EXISTS job_controls_decision_check,
    ADD CONSTRAINT job_controls_decision_check
        CHECK (
            decision IN (
                'accepted',
                'rejected-unauthorized',
                'rejected-duplicate',
                'rejected-unknown',
                'rejected-context'
            )
        );
