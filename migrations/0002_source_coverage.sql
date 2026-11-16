ALTER TABLE external_status.sources
    ADD COLUMN dependency TEXT NOT NULL DEFAULT '',
    ADD COLUMN coverage TEXT NOT NULL DEFAULT '',
    ADD COLUMN status_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN domain TEXT NOT NULL DEFAULT '',
    ADD COLUMN collection_mode TEXT NOT NULL DEFAULT 'automatic';

CREATE VIEW external_status.source_catalog AS
SELECT id, name, category, enabled, dependency, coverage, status_url, domain, collection_mode
FROM external_status.sources;
