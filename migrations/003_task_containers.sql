-- Task Containers Schema
-- Manages per-session Kubernetes pods for isolated code execution

-- Task containers table
CREATE TABLE IF NOT EXISTS task_containers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Session and user binding
    session_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    user_id UUID NOT NULL,

    -- Kubernetes resources
    pod_name TEXT NOT NULL,
    pod_namespace TEXT NOT NULL DEFAULT 'canal-workers',
    workspace_pvc TEXT,
    grpc_endpoint TEXT,

    -- Container status
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'creating', 'running', 'paused', 'terminating', 'terminated', 'error')),
    status_message TEXT,

    -- Resource limits
    cpu_limit TEXT DEFAULT '2000m',
    memory_limit TEXT DEFAULT '4Gi',
    storage_limit TEXT DEFAULT '10Gi',
    max_runtime_secs INTEGER DEFAULT 14400, -- 4 hours

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    last_activity TIMESTAMPTZ DEFAULT NOW(),
    timeout_at TIMESTAMPTZ,
    terminated_at TIMESTAMPTZ,

    -- Metadata
    metadata JSONB DEFAULT '{}'::jsonb
);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_task_containers_session ON task_containers(session_id);
CREATE INDEX IF NOT EXISTS idx_task_containers_user ON task_containers(user_id);
CREATE INDEX IF NOT EXISTS idx_task_containers_status ON task_containers(status)
    WHERE status IN ('pending', 'creating', 'running', 'paused');
CREATE INDEX IF NOT EXISTS idx_task_containers_pod ON task_containers(pod_name, pod_namespace);
CREATE INDEX IF NOT EXISTS idx_task_containers_timeout ON task_containers(timeout_at)
    WHERE status = 'running' AND timeout_at IS NOT NULL;

-- Container events table for audit log
CREATE TABLE IF NOT EXISTS container_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    container_id UUID NOT NULL REFERENCES task_containers(id) ON DELETE CASCADE,

    -- Event details
    event_type TEXT NOT NULL CHECK (event_type IN (
        'created', 'started', 'paused', 'resumed', 'terminated', 'error',
        'health_check_passed', 'health_check_failed',
        'resource_limit_reached', 'timeout_warning', 'timeout_reached'
    )),
    event_data JSONB,

    -- Timestamp
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for container events
CREATE INDEX IF NOT EXISTS idx_container_events_container ON container_events(container_id);
CREATE INDEX IF NOT EXISTS idx_container_events_type ON container_events(event_type);
CREATE INDEX IF NOT EXISTS idx_container_events_time ON container_events(created_at DESC);

-- Container usage tracking for billing/quotas
CREATE TABLE IF NOT EXISTS container_usage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    container_id UUID NOT NULL REFERENCES task_containers(id) ON DELETE CASCADE,
    user_id UUID NOT NULL,

    -- Usage period
    period_start TIMESTAMPTZ NOT NULL,
    period_end TIMESTAMPTZ NOT NULL,

    -- Resource usage
    cpu_seconds BIGINT DEFAULT 0,
    memory_gb_seconds BIGINT DEFAULT 0,
    storage_gb_hours DECIMAL(10, 4) DEFAULT 0,
    network_egress_mb BIGINT DEFAULT 0,

    -- Execution counts
    code_executions INTEGER DEFAULT 0,
    llm_requests INTEGER DEFAULT 0,
    tool_calls INTEGER DEFAULT 0,

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for usage tracking
CREATE INDEX IF NOT EXISTS idx_container_usage_container ON container_usage(container_id);
CREATE INDEX IF NOT EXISTS idx_container_usage_user ON container_usage(user_id);
CREATE INDEX IF NOT EXISTS idx_container_usage_period ON container_usage(period_start, period_end);

-- Function to update last_activity on container
CREATE OR REPLACE FUNCTION update_container_activity()
RETURNS TRIGGER AS $$
BEGIN
    UPDATE task_containers
    SET last_activity = NOW()
    WHERE id = NEW.container_id;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger to update activity on events (idempotent: drop first if exists)
DROP TRIGGER IF EXISTS trigger_container_activity ON container_events;
CREATE TRIGGER trigger_container_activity
    AFTER INSERT ON container_events
    FOR EACH ROW
    EXECUTE FUNCTION update_container_activity();

-- View for active containers with usage summary
CREATE OR REPLACE VIEW active_containers_summary AS
SELECT
    tc.id,
    tc.session_id,
    tc.user_id,
    tc.pod_name,
    tc.status,
    tc.cpu_limit,
    tc.memory_limit,
    tc.created_at,
    tc.last_activity,
    tc.timeout_at,
    EXTRACT(EPOCH FROM (NOW() - tc.created_at)) AS uptime_seconds,
    COALESCE(SUM(cu.code_executions), 0) AS total_code_executions,
    COALESCE(SUM(cu.llm_requests), 0) AS total_llm_requests,
    COUNT(ce.id) AS event_count
FROM task_containers tc
LEFT JOIN container_usage cu ON tc.id = cu.container_id
LEFT JOIN container_events ce ON tc.id = ce.container_id
WHERE tc.status IN ('pending', 'creating', 'running', 'paused')
GROUP BY tc.id;

-- Grant permissions (adjust based on your DB user)
-- GRANT SELECT, INSERT, UPDATE, DELETE ON task_containers TO canal_app;
-- GRANT SELECT, INSERT ON container_events TO canal_app;
-- GRANT SELECT, INSERT, UPDATE ON container_usage TO canal_app;
