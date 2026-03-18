-- Migration 013: Gift Card System
-- Production-grade gift card and top-up tracking

-- Gift Cards table
CREATE TABLE IF NOT EXISTS gift_cards (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code TEXT NOT NULL UNIQUE,
    amount_usd DOUBLE PRECISION NOT NULL,
    currency TEXT NOT NULL DEFAULT 'USD',
    status TEXT NOT NULL DEFAULT 'active',
    created_by UUID REFERENCES users(id),
    redeemed_by UUID REFERENCES users(id),
    redeemed_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    batch_id UUID,
    notes TEXT,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_gift_cards_code ON gift_cards(code);
CREATE INDEX IF NOT EXISTS idx_gift_cards_status ON gift_cards(status);
CREATE INDEX IF NOT EXISTS idx_gift_cards_batch ON gift_cards(batch_id);

-- Enhance billing_topups with source tracking
ALTER TABLE billing_topups ADD COLUMN IF NOT EXISTS source TEXT DEFAULT 'manual';
ALTER TABLE billing_topups ADD COLUMN IF NOT EXISTS source_reference TEXT;
ALTER TABLE billing_topups ADD COLUMN IF NOT EXISTS currency TEXT DEFAULT 'USD';
ALTER TABLE billing_topups ADD COLUMN IF NOT EXISTS status TEXT DEFAULT 'completed';
