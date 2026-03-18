-- Session Checkpoints Schema
-- Enables session pause/resume with state snapshots

-- Session checkpoints table
CREATE TABLE IF NOT EXISTS session_checkpoints (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id UUID NOT NULL,

    -- Checkpoint metadata
    checkpoint_name TEXT,
    checkpoint_type TEXT NOT NULL DEFAULT 'manual'
        CHECK (checkpoint_type IN ('manual', 'auto', 'pre_action', 'recovery')),

    -- Conversation state (messages, context)
    conversation_state JSONB NOT NULL,

    -- Workspace state
    workspace_snapshot_path TEXT,
    workspace_file_count INTEGER DEFAULT 0,
    workspace_size_bytes BIGINT DEFAULT 0,

    -- Container state reference
    container_id UUID REFERENCES task_containers(id) ON DELETE SET NULL,
    container_status TEXT,

    -- Metadata
    metadata JSONB DEFAULT '{}'::jsonb,

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ
);

-- Indexes for checkpoints
CREATE INDEX IF NOT EXISTS idx_session_checkpoints_session
    ON session_checkpoints(session_id);
CREATE INDEX IF NOT EXISTS idx_session_checkpoints_user
    ON session_checkpoints(user_id);
CREATE INDEX IF NOT EXISTS idx_session_checkpoints_created
    ON session_checkpoints(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_checkpoints_type
    ON session_checkpoints(checkpoint_type);

-- Session state table (for active session tracking)
CREATE TABLE IF NOT EXISTS session_states (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    user_id UUID NOT NULL,

    -- Current state
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'paused', 'expired', 'terminated')),

    -- Associated resources
    container_id UUID REFERENCES task_containers(id) ON DELETE SET NULL,
    workspace_path TEXT,

    -- Activity tracking
    last_message_at TIMESTAMPTZ,
    last_tool_call_at TIMESTAMPTZ,
    last_file_change_at TIMESTAMPTZ,

    -- Limits
    max_idle_minutes INTEGER DEFAULT 60,
    max_duration_hours INTEGER DEFAULT 24,

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    paused_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ
);

-- Indexes for session states
CREATE INDEX IF NOT EXISTS idx_session_states_user
    ON session_states(user_id);
CREATE INDEX IF NOT EXISTS idx_session_states_status
    ON session_states(status) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_session_states_expires
    ON session_states(expires_at) WHERE status = 'active';

-- Session files table (tracks file changes)
CREATE TABLE IF NOT EXISTS session_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,

    -- File info
    file_path TEXT NOT NULL,
    file_hash TEXT,
    file_size BIGINT DEFAULT 0,

    -- Change tracking
    change_type TEXT NOT NULL
        CHECK (change_type IN ('created', 'modified', 'deleted', 'renamed')),
    previous_path TEXT,  -- For renames
    previous_hash TEXT,  -- For modifications

    -- Metadata
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Unique constraint per session per path
    UNIQUE (session_id, file_path, created_at)
);

-- Indexes for session files
CREATE INDEX IF NOT EXISTS idx_session_files_session
    ON session_files(session_id);
CREATE INDEX IF NOT EXISTS idx_session_files_path
    ON session_files(file_path);

-- Function to update session state timestamp
CREATE OR REPLACE FUNCTION update_session_state_timestamp()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger for session state updates
DROP TRIGGER IF EXISTS trigger_session_state_updated ON session_states;
CREATE TRIGGER trigger_session_state_updated
    BEFORE UPDATE ON session_states
    FOR EACH ROW
    EXECUTE FUNCTION update_session_state_timestamp();

-- View for active sessions with details
CREATE OR REPLACE VIEW active_sessions_view AS
SELECT
    ss.id AS state_id,
    ss.session_id,
    ss.user_id,
    ss.status,
    ss.container_id,
    tc.pod_name,
    tc.status AS container_status,
    ss.created_at,
    ss.updated_at,
    ss.last_message_at,
    ss.expires_at,
    EXTRACT(EPOCH FROM (NOW() - ss.updated_at)) / 60 AS idle_minutes,
    (SELECT COUNT(*) FROM session_checkpoints sc WHERE sc.session_id = ss.session_id) AS checkpoint_count,
    (SELECT COUNT(*) FROM session_files sf WHERE sf.session_id = ss.session_id) AS file_change_count
FROM session_states ss
LEFT JOIN task_containers tc ON ss.container_id = tc.id
WHERE ss.status = 'active';

-- Function to create auto checkpoint before expiration
CREATE OR REPLACE FUNCTION create_expiration_checkpoint()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.status = 'active' AND NEW.status = 'expired' THEN
        INSERT INTO session_checkpoints (
            session_id, user_id, checkpoint_name, checkpoint_type,
            conversation_state, container_id, container_status
        )
        SELECT
            NEW.session_id,
            NEW.user_id,
            'Auto-save before expiration',
            'auto',
            '{"auto_saved": true}'::jsonb,
            NEW.container_id,
            NEW.status;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger for auto checkpoint on expiration
DROP TRIGGER IF EXISTS trigger_session_expiration_checkpoint ON session_states;
CREATE TRIGGER trigger_session_expiration_checkpoint
    BEFORE UPDATE ON session_states
    FOR EACH ROW
    WHEN (OLD.status = 'active' AND NEW.status = 'expired')
    EXECUTE FUNCTION create_expiration_checkpoint();
