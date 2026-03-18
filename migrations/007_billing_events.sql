-- Migration 007: Billing Events and Topups
-- Tracks per-request billing events and balance top-ups

-- Billing events for usage tracking
CREATE TABLE IF NOT EXISTS billing_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    pricing_plan_id TEXT,
    model_profile_id TEXT,
    provider TEXT,
    model TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    balance_before DOUBLE PRECISION,
    balance_after DOUBLE PRECISION,
    request_id UUID,
    metadata JSONB,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_billing_events_user_id ON billing_events(user_id);
CREATE INDEX IF NOT EXISTS idx_billing_events_timestamp ON billing_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_billing_events_user_date ON billing_events(user_id, timestamp);

-- Balance top-ups and credits
CREATE TABLE IF NOT EXISTS billing_topups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    amount_usd DOUBLE PRECISION NOT NULL,
    payment_method TEXT,
    payment_reference TEXT,
    balance_before DOUBLE PRECISION NOT NULL,
    balance_after DOUBLE PRECISION NOT NULL,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_billing_topups_user_id ON billing_topups(user_id);
