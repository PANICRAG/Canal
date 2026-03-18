-- Workflow Templates and Agent Checkpoints Schema
-- Version: 009
-- Created: 2026-01-28
-- Supports: Workflow recording, template learning, agent checkpoints

-- ==========================================
-- Workflow Templates (Learned Patterns)
-- ==========================================

-- Workflow templates for recorded and learned patterns
CREATE TABLE IF NOT EXISTS workflow_templates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Ownership
    owner_id UUID REFERENCES users(id) ON DELETE SET NULL,
    organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    team_id UUID,  -- For team sharing

    -- Template metadata
    name TEXT NOT NULL,
    description TEXT,
    version INTEGER DEFAULT 1,

    -- Template definition
    steps JSONB NOT NULL DEFAULT '[]',
    parameters JSONB DEFAULT '{}',
    conditions JSONB DEFAULT '[]',
    expected_outcomes JSONB DEFAULT '[]',

    -- Execution hints
    tool_categories JSONB DEFAULT '[]',  -- ["read_only", "reversible", "sensitive"]
    execution_strategy TEXT DEFAULT 'hybrid'
        CHECK (execution_strategy IN ('parallel', 'serial', 'hybrid')),
    estimated_duration_ms BIGINT,

    -- Learning metadata
    source_type TEXT NOT NULL DEFAULT 'manual'
        CHECK (source_type IN ('manual', 'recorded', 'learned', 'imported')),
    pattern_type TEXT,  -- e.g., "3_node_base_grade", "batch_normalize"
    recommended_conditions JSONB DEFAULT '[]',
    not_recommended_conditions JSONB DEFAULT '[]',

    -- Statistics
    success_rate REAL DEFAULT 0.0,
    execution_count INTEGER DEFAULT 0,
    total_time_saved_ms BIGINT DEFAULT 0,
    avg_execution_time_ms BIGINT,

    -- Visibility and sharing
    visibility TEXT DEFAULT 'private'
        CHECK (visibility IN ('private', 'team', 'organization', 'public')),
    is_featured BOOLEAN DEFAULT FALSE,

    -- Tags for discovery
    tags JSONB DEFAULT '[]',

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for workflow templates
CREATE INDEX IF NOT EXISTS idx_workflow_templates_owner
    ON workflow_templates(owner_id);
CREATE INDEX IF NOT EXISTS idx_workflow_templates_org
    ON workflow_templates(organization_id);
CREATE INDEX IF NOT EXISTS idx_workflow_templates_team
    ON workflow_templates(team_id);
CREATE INDEX IF NOT EXISTS idx_workflow_templates_visibility
    ON workflow_templates(visibility);
CREATE INDEX IF NOT EXISTS idx_workflow_templates_source
    ON workflow_templates(source_type);
CREATE INDEX IF NOT EXISTS idx_workflow_templates_success
    ON workflow_templates(success_rate DESC);
CREATE INDEX IF NOT EXISTS idx_workflow_templates_tags
    ON workflow_templates USING gin(tags);

-- Trigger for updated_at (idempotent: drop first if exists)
DROP TRIGGER IF EXISTS update_workflow_templates_updated_at ON workflow_templates;
CREATE TRIGGER update_workflow_templates_updated_at
    BEFORE UPDATE ON workflow_templates
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- ==========================================
-- Template Executions
-- ==========================================

-- Track template executions for learning
CREATE TABLE IF NOT EXISTS template_executions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    template_id UUID NOT NULL REFERENCES workflow_templates(id) ON DELETE CASCADE,
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,

    -- Execution details
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'paused', 'completed', 'failed', 'cancelled', 'rolled_back')),

    -- Input/Output
    input_params JSONB,
    output_results JSONB,

    -- Progress tracking
    total_steps INTEGER DEFAULT 0,
    completed_steps INTEGER DEFAULT 0,
    current_step TEXT,

    -- Performance metrics
    duration_ms BIGINT,
    retries INTEGER DEFAULT 0,

    -- Error handling
    error_type TEXT,
    error_message TEXT,
    recovery_attempted BOOLEAN DEFAULT FALSE,

    -- Timestamps
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for template executions
CREATE INDEX IF NOT EXISTS idx_template_executions_template
    ON template_executions(template_id);
CREATE INDEX IF NOT EXISTS idx_template_executions_user
    ON template_executions(user_id);
CREATE INDEX IF NOT EXISTS idx_template_executions_status
    ON template_executions(status);
CREATE INDEX IF NOT EXISTS idx_template_executions_created
    ON template_executions(created_at DESC);

-- ==========================================
-- Agent Checkpoints (Task-level)
-- ==========================================

-- Agent checkpoints for task execution
CREATE TABLE IF NOT EXISTS agent_checkpoints (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Context
    session_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,  -- Internal task reference
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,

    -- Checkpoint metadata
    checkpoint_name TEXT,
    checkpoint_type TEXT NOT NULL DEFAULT 'pre_action'
        CHECK (checkpoint_type IN ('pre_action', 'post_action', 'rollback_point', 'auto', 'manual')),

    -- State snapshot
    state_snapshot JSONB NOT NULL,
    tool_call JSONB,  -- The tool call this checkpoint protects

    -- Rollback capability
    can_rollback BOOLEAN DEFAULT TRUE,
    rollback_handler TEXT,  -- Name of rollback handler function
    rollback_params JSONB,
    was_rolled_back BOOLEAN DEFAULT FALSE,
    rolled_back_at TIMESTAMPTZ,

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ
);

-- Indexes for agent checkpoints
CREATE INDEX IF NOT EXISTS idx_agent_checkpoints_session
    ON agent_checkpoints(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_checkpoints_task
    ON agent_checkpoints(task_id);
CREATE INDEX IF NOT EXISTS idx_agent_checkpoints_type
    ON agent_checkpoints(checkpoint_type);
CREATE INDEX IF NOT EXISTS idx_agent_checkpoints_created
    ON agent_checkpoints(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_agent_checkpoints_rollback
    ON agent_checkpoints(can_rollback, was_rolled_back);

-- ==========================================
-- Team Workspaces
-- ==========================================

-- Team workspaces for collaboration
CREATE TABLE IF NOT EXISTS team_workspaces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE,

    -- Workspace metadata
    name TEXT NOT NULL,
    description TEXT,

    -- Settings
    preferences JSONB DEFAULT '{}',
    default_execution_strategy TEXT DEFAULT 'hybrid',
    auto_share_workflows BOOLEAN DEFAULT FALSE,

    -- Statistics
    member_count INTEGER DEFAULT 0,
    workflow_count INTEGER DEFAULT 0,
    total_executions INTEGER DEFAULT 0,

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for team workspaces
CREATE INDEX IF NOT EXISTS idx_team_workspaces_org
    ON team_workspaces(organization_id);

-- Trigger for updated_at (idempotent: drop first if exists)
DROP TRIGGER IF EXISTS update_team_workspaces_updated_at ON team_workspaces;
CREATE TRIGGER update_team_workspaces_updated_at
    BEFORE UPDATE ON team_workspaces
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- ==========================================
-- Team Workspace Members
-- ==========================================

CREATE TABLE IF NOT EXISTS team_workspace_members (
    workspace_id UUID NOT NULL REFERENCES team_workspaces(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    role TEXT NOT NULL DEFAULT 'member'
        CHECK (role IN ('owner', 'admin', 'member', 'viewer')),

    -- Permissions
    can_edit_workflows BOOLEAN DEFAULT TRUE,
    can_execute_workflows BOOLEAN DEFAULT TRUE,
    can_share_workflows BOOLEAN DEFAULT FALSE,
    can_manage_members BOOLEAN DEFAULT FALSE,

    -- Activity tracking
    last_active_at TIMESTAMPTZ,
    workflows_created INTEGER DEFAULT 0,
    workflows_executed INTEGER DEFAULT 0,

    -- Timestamps
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (workspace_id, user_id)
);

-- Indexes for team workspace members
CREATE INDEX IF NOT EXISTS idx_team_workspace_members_user
    ON team_workspace_members(user_id);

-- ==========================================
-- Workflow Ratings & Feedback
-- ==========================================

CREATE TABLE IF NOT EXISTS workflow_ratings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    template_id UUID NOT NULL REFERENCES workflow_templates(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    rating INTEGER NOT NULL CHECK (rating >= 1 AND rating <= 5),
    feedback TEXT,

    -- Execution context
    execution_id UUID REFERENCES template_executions(id) ON DELETE SET NULL,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- One rating per user per template
    UNIQUE (template_id, user_id)
);

-- Indexes for workflow ratings
CREATE INDEX IF NOT EXISTS idx_workflow_ratings_template
    ON workflow_ratings(template_id);
CREATE INDEX IF NOT EXISTS idx_workflow_ratings_user
    ON workflow_ratings(user_id);

-- ==========================================
-- Views
-- ==========================================

-- Popular templates view
CREATE OR REPLACE VIEW popular_templates_view AS
SELECT
    wt.id,
    wt.name,
    wt.description,
    wt.source_type,
    wt.pattern_type,
    wt.success_rate,
    wt.execution_count,
    wt.visibility,
    wt.tags,
    u.name AS owner_name,
    COALESCE(AVG(wr.rating), 0) AS avg_rating,
    COUNT(DISTINCT wr.user_id) AS rating_count
FROM workflow_templates wt
LEFT JOIN users u ON wt.owner_id = u.id
LEFT JOIN workflow_ratings wr ON wt.id = wr.template_id
WHERE wt.visibility IN ('team', 'organization', 'public')
GROUP BY wt.id, u.name
ORDER BY wt.execution_count DESC, wt.success_rate DESC;

-- User workflow stats view
CREATE OR REPLACE VIEW user_workflow_stats_view AS
SELECT
    u.id AS user_id,
    u.name AS user_name,
    COUNT(DISTINCT wt.id) AS templates_created,
    COUNT(DISTINCT te.id) AS total_executions,
    AVG(wt.success_rate) AS avg_success_rate,
    SUM(wt.total_time_saved_ms) / 1000 / 60 AS total_time_saved_minutes
FROM users u
LEFT JOIN workflow_templates wt ON u.id = wt.owner_id
LEFT JOIN template_executions te ON u.id = te.user_id
GROUP BY u.id, u.name;

-- ==========================================
-- Functions
-- ==========================================

-- Function to update template statistics after execution
CREATE OR REPLACE FUNCTION update_template_stats()
RETURNS TRIGGER AS $$
DECLARE
    success_count INTEGER;
    total_count INTEGER;
    avg_time BIGINT;
BEGIN
    IF NEW.status IN ('completed', 'failed') THEN
        -- Calculate new statistics
        SELECT
            COUNT(*) FILTER (WHERE status = 'completed'),
            COUNT(*),
            AVG(duration_ms) FILTER (WHERE status = 'completed')
        INTO success_count, total_count, avg_time
        FROM template_executions
        WHERE template_id = NEW.template_id;

        -- Update template
        UPDATE workflow_templates
        SET
            success_rate = CASE WHEN total_count > 0
                THEN success_count::REAL / total_count
                ELSE 0 END,
            execution_count = total_count,
            avg_execution_time_ms = avg_time
        WHERE id = NEW.template_id;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger for template stats update (idempotent: drop first if exists)
DROP TRIGGER IF EXISTS trigger_update_template_stats ON template_executions;
CREATE TRIGGER trigger_update_template_stats
    AFTER UPDATE ON template_executions
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status)
    EXECUTE FUNCTION update_template_stats();

-- Function to clean up expired checkpoints
CREATE OR REPLACE FUNCTION cleanup_expired_checkpoints()
RETURNS INTEGER AS $$
DECLARE
    deleted_count INTEGER;
BEGIN
    WITH deleted AS (
        DELETE FROM agent_checkpoints
        WHERE expires_at < NOW()
        RETURNING id
    )
    SELECT COUNT(*) INTO deleted_count FROM deleted;

    RETURN deleted_count;
END;
$$ LANGUAGE plpgsql;
