-- Context Engineering v2 Migration
-- Adds config versioning and audit trail for organizations

-- Add config versioning to organizations table
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS config_version INTEGER DEFAULT 1;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS config_snapshot JSONB;

-- Audit trail for organization config changes
CREATE TABLE IF NOT EXISTS organization_config_audit (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL,
    config_version INTEGER NOT NULL,
    config_snapshot JSONB NOT NULL,
    changed_by UUID,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    change_description TEXT
);

CREATE INDEX IF NOT EXISTS idx_org_config_audit_org_id
    ON organization_config_audit(organization_id);
CREATE INDEX IF NOT EXISTS idx_org_config_audit_changed_at
    ON organization_config_audit(changed_at DESC);

-- Add claude_md_hash to users table for change detection (ADR-19)
ALTER TABLE users ADD COLUMN IF NOT EXISTS claude_md_hash VARCHAR(64);
