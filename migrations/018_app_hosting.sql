-- App hosting tables (CP29)

CREATE TABLE IF NOT EXISTS hosted_apps (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id),
    name VARCHAR(255) NOT NULL,
    subdomain VARCHAR(255) NOT NULL UNIQUE,
    framework VARCHAR(50) NOT NULL DEFAULT 'other',
    status VARCHAR(50) NOT NULL DEFAULT 'draft',
    deploy_config JSONB NOT NULL DEFAULT '{}',
    current_deployment_id UUID,
    container_id VARCHAR(255),
    port INTEGER,
    custom_domain VARCHAR(255),
    domain_verified BOOLEAN NOT NULL DEFAULT FALSE,
    created_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_hosted_apps_org_id ON hosted_apps(org_id);
CREATE INDEX IF NOT EXISTS idx_hosted_apps_subdomain ON hosted_apps(subdomain);
CREATE INDEX IF NOT EXISTS idx_hosted_apps_status ON hosted_apps(status);

CREATE TABLE IF NOT EXISTS app_deployments (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES hosted_apps(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    image_tag VARCHAR(255) NOT NULL,
    commit_sha VARCHAR(64),
    status VARCHAR(50) NOT NULL DEFAULT 'queued',
    logs TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    UNIQUE(app_id, version)
);

CREATE INDEX IF NOT EXISTS idx_app_deployments_app_id ON app_deployments(app_id);

CREATE TABLE IF NOT EXISTS app_builds (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES hosted_apps(id) ON DELETE CASCADE,
    deployment_id UUID NOT NULL REFERENCES app_deployments(id) ON DELETE CASCADE,
    engine VARCHAR(50) NOT NULL DEFAULT 'nixpacks',
    status VARCHAR(50) NOT NULL DEFAULT 'queued',
    image_tag VARCHAR(255),
    logs TEXT NOT NULL DEFAULT '',
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_app_builds_app_id ON app_builds(app_id);
CREATE INDEX IF NOT EXISTS idx_app_builds_deployment_id ON app_builds(deployment_id);
