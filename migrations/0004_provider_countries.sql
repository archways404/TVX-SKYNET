ALTER TABLE external_status.sources
    ADD COLUMN IF NOT EXISTS country_codes TEXT[] NOT NULL DEFAULT '{}';

CREATE OR REPLACE VIEW external_status.source_catalog AS
SELECT id, name, category, enabled, dependency, coverage, status_url, domain, collection_mode, country_codes
FROM external_status.sources;
