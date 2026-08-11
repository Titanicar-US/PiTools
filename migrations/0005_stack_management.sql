CREATE TABLE IF NOT EXISTS pull_request_stack_relationships (
    repository_id BIGINT NOT NULL,
    pull_request_number INTEGER NOT NULL,
    parent_pull_request_number INTEGER,
    head_branch TEXT NOT NULL,
    observed_base_branch TEXT NOT NULL,
    relationship_source TEXT NOT NULL CHECK (relationship_source IN ('explicit', 'base_inference')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repository_id, pull_request_number),
    FOREIGN KEY (repository_id, pull_request_number)
        REFERENCES pull_requests(repository_id, number) ON DELETE CASCADE,
    FOREIGN KEY (repository_id, parent_pull_request_number)
        REFERENCES pull_requests(repository_id, number) ON DELETE CASCADE,
    CHECK (parent_pull_request_number IS NULL OR parent_pull_request_number <> pull_request_number)
);

CREATE INDEX IF NOT EXISTS idx_stack_relationships_parent
    ON pull_request_stack_relationships (repository_id, parent_pull_request_number);

CREATE TABLE IF NOT EXISTS branch_rewrite_policies (
    repository_id BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    branch_name TEXT NOT NULL,
    bot_owned BOOLEAN NOT NULL DEFAULT FALSE,
    allow_history_rewrite BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repository_id, branch_name),
    CHECK (NOT allow_history_rewrite OR bot_owned)
);
