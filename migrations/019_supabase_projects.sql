-- CP38: Managed database (Supabase BaaS) project tracking.

CREATE TABLE IF NOT EXISTS supabase_projects (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES hosted_apps(id) ON DELETE CASCADE,
    org_id UUID NOT NULL,
    project_ref TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    region TEXT NOT NULL DEFAULT 'ap-southeast-1',
    status TEXT NOT NULL DEFAULT 'creating',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_supabase_projects_app_id ON supabase_projects(app_id);
CREATE INDEX IF NOT EXISTS idx_supabase_projects_org_id ON supabase_projects(org_id);
CREATE INDEX IF NOT EXISTS idx_supabase_projects_status ON supabase_projects(status);
