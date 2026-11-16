-- Existing rows retain an empty object; existing SELECT grants on the view survive replacement.
ALTER TABLE external_status.incidents
    ADD COLUMN IF NOT EXISTS network_details JSONB NOT NULL DEFAULT '{}';

CREATE OR REPLACE VIEW external_status.active_incidents AS
SELECT provider_id, external_id, title, lifecycle, impact, started_at,
       provider_updated_at, source_url, affected_component_ids, network_details
FROM external_status.incidents
WHERE resolved_at IS NULL
  AND lifecycle NOT IN ('resolved', 'completed');
