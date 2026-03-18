-- Migration 006: Billing Plans
-- Adds pricing plans, user billing fields, and API key enhancements

-- Add billing fields to users
ALTER TABLE users ADD COLUMN IF NOT EXISTS pricing_plan_id TEXT DEFAULT 'free-trial';
ALTER TABLE users ADD COLUMN IF NOT EXISTS balance_usd DOUBLE PRECISION DEFAULT 0.0;
ALTER TABLE users ADD COLUMN IF NOT EXISTS credit_limit_usd DOUBLE PRECISION;
ALTER TABLE users ADD COLUMN IF NOT EXISTS monthly_budget_limit_usd DOUBLE PRECISION;

-- Pricing plans table
CREATE TABLE IF NOT EXISTS pricing_plans (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mode TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    config JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Enhance API keys with profile and rate limit controls
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS allowed_profiles JSONB;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS rate_limit_rpm INTEGER;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS enabled BOOLEAN DEFAULT true;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS tier TEXT DEFAULT 'free';
