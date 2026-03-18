-- Migration 008: Billing Invoices
-- Monthly invoice generation for subscription and usage billing

CREATE TABLE IF NOT EXISTS billing_invoices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    period_start TIMESTAMPTZ NOT NULL,
    period_end TIMESTAMPTZ NOT NULL,
    pricing_plan_id TEXT,
    subscription_fee_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    usage_fee_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    overage_fee_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    total_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    total_requests INTEGER NOT NULL DEFAULT 0,
    total_input_tokens BIGINT NOT NULL DEFAULT 0,
    total_output_tokens BIGINT NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'draft',
    paid_at TIMESTAMPTZ,
    payment_reference TEXT,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_billing_invoices_user_id ON billing_invoices(user_id);
CREATE INDEX IF NOT EXISTS idx_billing_invoices_period ON billing_invoices(period_start, period_end);
