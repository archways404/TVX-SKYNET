ALTER TABLE external_status.sources
    ADD COLUMN IF NOT EXISTS dependency TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS coverage TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS status_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS domain TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS collection_mode TEXT NOT NULL DEFAULT 'automatic';

CREATE OR REPLACE VIEW external_status.source_catalog AS
SELECT id, name, category, enabled, dependency, coverage, status_url, domain, collection_mode
FROM external_status.sources;
