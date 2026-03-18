-- CP37: Custom domain verification tracking.

CREATE TABLE IF NOT EXISTS domain_verifications (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES hosted_apps(id) ON DELETE CASCADE,
    domain TEXT NOT NULL,
    txt_record_name TEXT NOT NULL,
    txt_record_value TEXT NOT NULL,
    cname_target TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    verified_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_domain_verifications_app_id ON domain_verifications(app_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_domain_verifications_domain ON domain_verifications(domain);
CREATE INDEX IF NOT EXISTS idx_domain_verifications_status ON domain_verifications(status);
