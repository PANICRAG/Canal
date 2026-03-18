-- CP39: Object storage metadata tracking.

CREATE TABLE IF NOT EXISTS storage_objects (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES hosted_apps(id) ON DELETE CASCADE,
    org_id UUID NOT NULL,
    key TEXT NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    size_bytes BIGINT NOT NULL DEFAULT 0,
    etag TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(app_id, key)
);

CREATE INDEX IF NOT EXISTS idx_storage_objects_app_id ON storage_objects(app_id);
CREATE INDEX IF NOT EXISTS idx_storage_objects_org_id ON storage_objects(org_id);
