-- CP48: Auth Hardening — Refresh Token Families
--
-- Replaces flat refresh_tokens with rotation-aware families.
-- Supports replay detection via generation counter.

CREATE TABLE IF NOT EXISTS refresh_token_families (
    session_id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    current_generation INTEGER NOT NULL DEFAULT 0,
    current_token_hash TEXT NOT NULL,
    ip_address TEXT,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_rotated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    revoked_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_refresh_families_user
    ON refresh_token_families(user_id);

CREATE INDEX IF NOT EXISTS idx_refresh_families_active
    ON refresh_token_families(expires_at)
    WHERE NOT revoked;

-- Service accounts for inter-service auth (gateway-api, devtools-server, etc.)
CREATE TABLE IF NOT EXISTS service_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_name TEXT NOT NULL UNIQUE,
    description TEXT,
    scopes JSONB NOT NULL DEFAULT '[]'::JSONB,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ
);

-- Seed default service accounts for dev mode
INSERT INTO service_accounts (service_name, description, scopes)
VALUES
    ('gateway-api', 'AI Engine — needs hosting and instance access',
     '["hosting:*", "instances:*", "apps:*"]'::JSONB),
    ('devtools-server', 'DevTools observation server',
     '["metrics:read", "alerts:*", "logs:read"]'::JSONB)
ON CONFLICT (service_name) DO NOTHING;
