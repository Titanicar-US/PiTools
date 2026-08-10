CREATE TABLE IF NOT EXISTS installations (
    id BIGINT PRIMARY KEY,
    account_login TEXT NOT NULL,
    account_type TEXT NOT NULL,
    suspended_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS repositories (
    id BIGINT PRIMARY KEY,
    installation_id BIGINT NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    default_branch TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    policy_yaml TEXT NOT NULL DEFAULT '',
    policy_revision TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (installation_id, owner, name)
);

CREATE TABLE IF NOT EXISTS pull_requests (
    repository_id BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    github_id BIGINT NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    state TEXT NOT NULL,
    draft BOOLEAN NOT NULL DEFAULT FALSE,
    merged BOOLEAN NOT NULL DEFAULT FALSE,
    head_sha TEXT NOT NULL,
    base_sha TEXT NOT NULL,
    head_branch TEXT NOT NULL,
    base_branch TEXT NOT NULL,
    author_login TEXT NOT NULL,
    readiness JSONB,
    watched BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repository_id, number),
    UNIQUE (github_id)
);

CREATE TABLE IF NOT EXISTS event_deliveries (
    delivery_id TEXT PRIMARY KEY,
    event_name TEXT NOT NULL,
    action TEXT,
    installation_id BIGINT,
    repository_id BIGINT,
    pull_request_number INTEGER,
    payload_hash TEXT NOT NULL,
    payload JSONB NOT NULL,
    raw_payload BYTEA,
    raw_payload_expires_at TIMESTAMPTZ,
    supported BOOLEAN NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at TIMESTAMPTZ,
    processing_error TEXT
);

CREATE TABLE IF NOT EXISTS feedback_items (
    id TEXT PRIMARY KEY,
    repository_id BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    pull_request_number INTEGER NOT NULL,
    actor_login TEXT NOT NULL,
    actor_type TEXT NOT NULL,
    body TEXT NOT NULL,
    resolved BOOLEAN NOT NULL DEFAULT FALSE,
    is_automation BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS checks (
    repository_id BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    pull_request_number INTEGER NOT NULL,
    external_id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    conclusion TEXT,
    required BOOLEAN NOT NULL DEFAULT FALSE,
    details_url TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repository_id, pull_request_number, external_id)
);

CREATE TABLE IF NOT EXISTS jobs (
    id UUID PRIMARY KEY,
    repository_id BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    pull_request_number INTEGER NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    current_item TEXT,
    plan JSONB NOT NULL DEFAULT '{}'::jsonb,
    result JSONB,
    attempt INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS job_controls (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    action TEXT NOT NULL,
    actor_login TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (job_id, action, actor_login)
);

CREATE TABLE IF NOT EXISTS comments (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    github_comment_id BIGINT,
    kind TEXT NOT NULL,
    marker TEXT NOT NULL,
    body_hash TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (job_id, kind)
);

CREATE TABLE IF NOT EXISTS readiness_snapshots (
    id UUID PRIMARY KEY,
    repository_id BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    pull_request_number INTEGER NOT NULL,
    ready BOOLEAN NOT NULL,
    reason_codes JSONB NOT NULL,
    evaluated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS audit_entries (
    id UUID PRIMARY KEY,
    repository_id BIGINT,
    pull_request_number INTEGER,
    job_id UUID,
    actor_login TEXT,
    event_type TEXT NOT NULL,
    summary TEXT NOT NULL,
    evidence JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_pull_requests_watched ON pull_requests (watched, updated_at);
CREATE INDEX IF NOT EXISTS idx_event_deliveries_received ON event_deliveries (received_at);
CREATE INDEX IF NOT EXISTS idx_jobs_state_lease ON jobs (state, lease_until);
CREATE INDEX IF NOT EXISTS idx_audit_entries_pr ON audit_entries (repository_id, pull_request_number, created_at);
