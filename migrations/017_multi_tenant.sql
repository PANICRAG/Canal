-- Migration 017: Multi-Tenant Platform (CP26.1)
-- Adds organizations, org_members, org_invitations, instances, instance_keys,
-- agent_identities, agent_api_keys, port_allocations, and audit_log tables.

-- ===== Extend organizations table =====
-- organizations table already exists from migration 001.
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS slug TEXT UNIQUE;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS plan_id TEXT NOT NULL DEFAULT 'free';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}';

-- ===== org_members =====
-- org_members may already exist from migration 001 with organization_id column.
-- Add missing columns if table already exists.
ALTER TABLE org_members ADD COLUMN IF NOT EXISTS invited_by UUID REFERENCES users(id);
ALTER TABLE org_members ADD COLUMN IF NOT EXISTS joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

-- ===== org_invitations =====
-- org_invitations may already exist from migration 001 with organization_id column.
ALTER TABLE org_invitations ADD COLUMN IF NOT EXISTS token_hash TEXT;
ALTER TABLE org_invitations ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE org_invitations ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '7 days';
DO $$ BEGIN
    CREATE INDEX IF NOT EXISTS idx_org_invitations_org ON org_invitations(organization_id);
EXCEPTION WHEN undefined_column THEN
    NULL;
END $$;

-- ===== instances =====
CREATE TABLE IF NOT EXISTS instances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    container_id TEXT,
    port INTEGER,
    status TEXT NOT NULL DEFAULT 'pending',
    config JSONB NOT NULL DEFAULT '{}',
    service_agent_id UUID,
    created_by UUID REFERENCES users(id),
    started_at TIMESTAMPTZ,
    health JSONB NOT NULL DEFAULT '{"healthy": false}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Defensive: ensure columns exist if table was pre-created with different schema
-- (e.g., by supabase/migrations or a previous deployment)
ALTER TABLE instances ADD COLUMN IF NOT EXISTS organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE instances ADD COLUMN IF NOT EXISTS name TEXT DEFAULT '';
ALTER TABLE instances ADD COLUMN IF NOT EXISTS container_id TEXT;
ALTER TABLE instances ADD COLUMN IF NOT EXISTS port INTEGER;
ALTER TABLE instances ADD COLUMN IF NOT EXISTS status TEXT DEFAULT 'pending';
ALTER TABLE instances ADD COLUMN IF NOT EXISTS config JSONB DEFAULT '{}';
ALTER TABLE instances ADD COLUMN IF NOT EXISTS service_agent_id UUID;
ALTER TABLE instances ADD COLUMN IF NOT EXISTS created_by UUID;
ALTER TABLE instances ADD COLUMN IF NOT EXISTS started_at TIMESTAMPTZ;
ALTER TABLE instances ADD COLUMN IF NOT EXISTS health JSONB DEFAULT '{"healthy": false}';
ALTER TABLE instances ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ DEFAULT NOW();
ALTER TABLE instances ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_instances_org ON instances(organization_id);
CREATE INDEX IF NOT EXISTS idx_instances_status ON instances(status);

-- ===== instance_keys =====
CREATE TABLE IF NOT EXISTS instance_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    instance_id UUID REFERENCES instances(id) ON DELETE CASCADE,
    agent_id UUID NOT NULL,
    key_prefix TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT 'default',
    scopes JSONB NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'active',
    created_by UUID REFERENCES users(id),
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- Defensive: ensure columns exist if table was pre-created with different schema
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS instance_id UUID;
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS agent_id UUID;
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS key_prefix TEXT DEFAULT '';
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS name TEXT DEFAULT 'default';
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS scopes JSONB DEFAULT '[]';
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS status TEXT DEFAULT 'active';
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS created_by UUID;
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;
ALTER TABLE instance_keys ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_instance_keys_instance ON instance_keys(instance_id);
CREATE INDEX IF NOT EXISTS idx_instance_keys_agent ON instance_keys(agent_id);

-- ===== agent_identities =====
CREATE TABLE IF NOT EXISTS agent_identities (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    tier TEXT NOT NULL DEFAULT 'Standard',
    scopes JSONB NOT NULL DEFAULT '[]',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
ALTER TABLE agent_identities ADD COLUMN IF NOT EXISTS name TEXT DEFAULT '';
ALTER TABLE agent_identities ADD COLUMN IF NOT EXISTS tier TEXT DEFAULT 'Standard';
ALTER TABLE agent_identities ADD COLUMN IF NOT EXISTS scopes JSONB DEFAULT '[]';
ALTER TABLE agent_identities ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ DEFAULT NOW();

-- ===== agent_api_keys =====
CREATE TABLE IF NOT EXISTS agent_api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agent_identities(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    key_hash TEXT UNIQUE NOT NULL,
    key_prefix TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    custom_scopes JSONB,
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS agent_id UUID;
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS name TEXT DEFAULT '';
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS key_hash TEXT;
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS key_prefix TEXT DEFAULT '';
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS status TEXT DEFAULT 'active';
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS custom_scopes JSONB;
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;
ALTER TABLE agent_api_keys ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_agent_keys_hash ON agent_api_keys(key_hash);

-- ===== port_allocations =====
CREATE TABLE IF NOT EXISTS port_allocations (
    port INTEGER PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    allocated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
ALTER TABLE port_allocations ADD COLUMN IF NOT EXISTS instance_id UUID;
ALTER TABLE port_allocations ADD COLUMN IF NOT EXISTS allocated_at TIMESTAMPTZ DEFAULT NOW();

-- ===== audit_log =====
CREATE TABLE IF NOT EXISTS audit_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID REFERENCES organizations(id),
    user_id UUID REFERENCES users(id),
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id UUID,
    details JSONB,
    ip_address TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- Defensive: ensure columns exist if table was pre-created with different schema
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS organization_id UUID;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS user_id UUID;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS action TEXT DEFAULT '';
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS resource_type TEXT DEFAULT '';
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS resource_id UUID;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS details JSONB;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS ip_address TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_audit_org_time ON audit_log(organization_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_user ON audit_log(user_id, created_at DESC);
