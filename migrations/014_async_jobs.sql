-- Async Job System (A18 Jobs Module)
-- Persistent background job execution for long-running agent tasks.

-- Custom enum types for job status and type
DO $$ BEGIN
    CREATE TYPE job_status AS ENUM (
        'submitted', 'queued', 'running', 'paused',
        'completed', 'failed', 'cancelled'
    );
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    CREATE TYPE job_type AS ENUM ('chat', 'collaboration', 'workflow');
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;

-- Main jobs table
CREATE TABLE IF NOT EXISTS jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    session_id UUID,
    job_type job_type NOT NULL DEFAULT 'chat',
    status job_status NOT NULL DEFAULT 'submitted',
    input JSONB NOT NULL,
    result JSONB,
    error TEXT,
    checkpoint_id TEXT,
    execution_id TEXT,
    progress_pct REAL,
    metadata JSONB NOT NULL DEFAULT '{}',
    notify_webhook TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for listing user jobs filtered by status
CREATE INDEX IF NOT EXISTS idx_jobs_user_status ON jobs (user_id, status);

-- Index for scheduler claim query (status + created_at ordering)
CREATE INDEX IF NOT EXISTS idx_jobs_status_created ON jobs (status, created_at);

-- Index for looking up jobs by execution_id (SSE stream lookup)
CREATE INDEX IF NOT EXISTS idx_jobs_execution_id ON jobs (execution_id) WHERE execution_id IS NOT NULL;
