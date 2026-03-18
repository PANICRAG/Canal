-- Context Engineering v2 Rollback Migration

ALTER TABLE users DROP COLUMN IF EXISTS claude_md_hash;

DROP TABLE IF EXISTS organization_config_audit;

ALTER TABLE organizations DROP COLUMN IF EXISTS config_snapshot;
ALTER TABLE organizations DROP COLUMN IF EXISTS config_version;
