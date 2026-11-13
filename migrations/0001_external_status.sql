CREATE TABLE external_status.sources (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    stale_after_seconds BIGINT NOT NULL CHECK (stale_after_seconds > 0),
    current_status TEXT NOT NULL DEFAULT 'unknown' CHECK (
        current_status IN (
            'operational',
            'maintenance',
            'degraded',
            'partial_outage',
            'major_outage',
            'unknown'
        )
    ),
    last_checked_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE external_status.components (
    provider_id TEXT NOT NULL REFERENCES external_status.sources(id) ON DELETE CASCADE,
    external_id TEXT NOT NULL,
    name TEXT NOT NULL,
    current_status TEXT NOT NULL CHECK (
        current_status IN (
            'operational',
            'maintenance',
            'degraded',
            'partial_outage',
            'major_outage',
            'unknown'
        )
    ),
    provider_updated_at TIMESTAMPTZ NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (provider_id, external_id)
);

CREATE TABLE external_status.incidents (
    provider_id TEXT NOT NULL REFERENCES external_status.sources(id) ON DELETE CASCADE,
    external_id TEXT NOT NULL,
    title TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN (
            'investigating',
            'identified',
            'monitoring',
            'resolved',
            'scheduled',
            'in_progress',
            'completed',
            'unknown'
        )
    ),
    impact TEXT NOT NULL CHECK (
        impact IN (
            'operational',
            'maintenance',
            'degraded',
            'partial_outage',
            'major_outage',
            'unknown'
        )
    ),
    started_at TIMESTAMPTZ NOT NULL,
    provider_updated_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    source_url TEXT,
    affected_component_ids TEXT[] NOT NULL DEFAULT '{}',
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (provider_id, external_id)
);

CREATE TABLE external_status.incident_updates (
    provider_id TEXT NOT NULL,
    incident_external_id TEXT NOT NULL,
    external_id TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN (
            'investigating',
            'identified',
            'monitoring',
            'resolved',
            'scheduled',
            'in_progress',
            'completed',
            'unknown'
        )
    ),
    message TEXT NOT NULL,
    published_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (provider_id, incident_external_id, external_id),
    FOREIGN KEY (provider_id, incident_external_id)
        REFERENCES external_status.incidents(provider_id, external_id)
        ON DELETE CASCADE
);

CREATE TABLE external_status.provider_observations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES external_status.sources(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'operational',
            'maintenance',
            'degraded',
            'partial_outage',
            'major_outage',
            'unknown'
        )
    )
);

CREATE TABLE external_status.component_observations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider_id TEXT NOT NULL,
    component_external_id TEXT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'operational',
            'maintenance',
            'degraded',
            'partial_outage',
            'major_outage',
            'unknown'
        )
    ),
    FOREIGN KEY (provider_id, component_external_id)
        REFERENCES external_status.components(provider_id, external_id)
        ON DELETE CASCADE
);

CREATE TABLE external_status.poll_attempts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES external_status.sources(id) ON DELETE CASCADE,
    checked_at TIMESTAMPTZ NOT NULL,
    succeeded BOOLEAN NOT NULL,
    error TEXT
);

CREATE INDEX provider_observations_provider_time_idx
    ON external_status.provider_observations (provider_id, observed_at DESC);

CREATE INDEX component_observations_component_time_idx
    ON external_status.component_observations (
        provider_id,
        component_external_id,
        observed_at DESC
    );

CREATE INDEX incidents_active_idx
    ON external_status.incidents (provider_id, provider_updated_at DESC)
    WHERE resolved_at IS NULL;

CREATE INDEX poll_attempts_source_time_idx
    ON external_status.poll_attempts (source_id, checked_at DESC);

CREATE VIEW external_status.provider_current AS
SELECT
    id,
    name,
    category,
    current_status,
    CASE
        WHEN last_success_at IS NULL THEN TRUE
        ELSE now() - last_success_at > stale_after_seconds * interval '1 second'
    END AS stale,
    CASE
        WHEN last_success_at IS NULL THEN 'unknown'
        WHEN now() - last_success_at > stale_after_seconds * interval '1 second' THEN 'unknown'
        ELSE current_status
    END AS effective_status,
    last_checked_at,
    last_success_at,
    consecutive_failures,
    last_error
FROM external_status.sources
WHERE enabled;

CREATE VIEW external_status.active_incidents AS
SELECT
    provider_id,
    external_id,
    title,
    lifecycle,
    impact,
    started_at,
    provider_updated_at,
    source_url,
    affected_component_ids
FROM external_status.incidents
WHERE resolved_at IS NULL
  AND lifecycle NOT IN ('resolved', 'completed');
