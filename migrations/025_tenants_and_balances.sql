-- Migration 025: Create tenants and user_balances tables
-- Fixes platform_get_status (admin overview) and billing metering errors.

-- ===== tenants =====
-- Used by PgTenantStore for multi-tenant platform management.
CREATE TABLE IF NOT EXISTS tenants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    plan_id TEXT NOT NULL DEFAULT 'free',
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tenants_user_id ON tenants(user_id);
CREATE INDEX IF NOT EXISTS idx_tenants_status ON tenants(status);

-- ===== user_balances =====
-- Used by billing-core PgBillingStore for token metering.
CREATE TABLE IF NOT EXISTS user_balances (
    user_id UUID PRIMARY KEY REFERENCES users(id),
    balance_mpt BIGINT NOT NULL DEFAULT 0,
    plan_id TEXT NOT NULL DEFAULT 'free',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ===== default organization for hosting =====
-- hosted_apps.org_id references organizations(id); ensure a default org exists
-- so that apps created without an explicit org_id (via chat tools) don't hit FK errors.
INSERT INTO organizations (id, name, slug)
VALUES ('00000000-0000-0000-0000-000000000001', 'Default', 'default')
ON CONFLICT (id) DO NOTHING;
