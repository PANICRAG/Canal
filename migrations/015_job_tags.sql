-- Add tags column to jobs table for LLM-generated classification
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS tags TEXT[] DEFAULT '{}';
CREATE INDEX IF NOT EXISTS idx_jobs_tags ON jobs USING GIN (tags);
