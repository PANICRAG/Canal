-- A38: Semantic Memory — pgvector-backed persistent memory storage
-- Requires: pgvector extension

CREATE EXTENSION IF NOT EXISTS vector;

-- Memory entries with optional embedding column for semantic search
CREATE TABLE IF NOT EXISTS memory_entries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    category TEXT NOT NULL,
    title TEXT,
    content TEXT NOT NULL,
    structured_data JSONB,
    tags TEXT[] DEFAULT '{}',
    confidence TEXT DEFAULT 'medium',
    source TEXT DEFAULT 'system',
    session_id UUID,
    metadata JSONB DEFAULT '{}',
    embedding vector(1536),
    version INTEGER DEFAULT 1,
    access_count INTEGER DEFAULT 0,
    last_accessed TIMESTAMPTZ DEFAULT NOW(),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(user_id, key)
);

-- Index for user-scoped queries
CREATE INDEX IF NOT EXISTS idx_memory_entries_user ON memory_entries(user_id);

-- Index for category filtering
CREATE INDEX IF NOT EXISTS idx_memory_entries_category ON memory_entries(user_id, category);

-- HNSW index for fast approximate nearest neighbor search on embeddings
CREATE INDEX IF NOT EXISTS idx_memory_entries_embedding ON memory_entries
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Conversation summaries with embedding for cross-conversation search
CREATE TABLE IF NOT EXISTS conversation_summaries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    summary TEXT NOT NULL,
    key_facts JSONB DEFAULT '[]',
    topics TEXT[] DEFAULT '{}',
    embedding vector(1536),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(conversation_id)
);

-- HNSW index for conversation summary embeddings
CREATE INDEX IF NOT EXISTS idx_conv_summaries_embedding ON conversation_summaries
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Index for user-scoped conversation summary queries
CREATE INDEX IF NOT EXISTS idx_conv_summaries_user ON conversation_summaries(user_id);
