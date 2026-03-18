-- Single-use invite codes for gated registration.

CREATE TABLE IF NOT EXISTS invite_codes (
    code TEXT PRIMARY KEY,
    used_by UUID REFERENCES platform_users(id),
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Seed default invite code
INSERT INTO invite_codes (code) VALUES ('RIVER-EARLY-2026')
    ON CONFLICT (code) DO NOTHING;
