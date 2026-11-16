# Grafana dashboard

`tvx-skynet-dashboard.json` is an importable SOC dashboard for the
`external_status` PostgreSQL schema.

## Import

1. Open **Dashboards** in Grafana.
2. Select **New**, then **Import**.
3. Upload `tvx-skynet-dashboard.json`.
4. Map **TVX-SKYNET PostgreSQL** to the read-only PostgreSQL datasource.
5. Select **Import**.

The dashboard does not hard-code a datasource UID. Grafana asks for the
datasource during import and replaces the `${DS_TVX_SKYNET}` input.

## Panels

- Overall external dependency state.
- Affected-provider roster with logos, current state, incident count and
  collection freshness.
- Current provider state and last successful poll.
- Active incident lifecycle and source link.
- Poll success rate in the selected time range.
- Provider state timeline.
- Provider-filtered component state.
- Provider-filtered recent poll attempts.

The dashboard defaults to the last 24 hours and refreshes once per minute. The
underlying collector currently polls every five minutes.

Provider logos are loaded by the browser from Google's favicon service. Provider
names and status text remain visible if external images are blocked by Grafana
content-security policy or by the client network.

When this file changes, import it again with the same dashboard UID and choose
**Overwrite** to update the existing dashboard while preserving its URL.


## Responsive v2 dashboard

Use `tvx-skynet-dashboard-v2.json` with the Grafana v2 code editor. The export uses
numeric base thresholds because this installation rejects `null` in that field.
`python3 scripts/build-dashboard-v2.py` regenerates it from the classic file.
`python3 scripts/check-dashboard.py` checks those types and the layout references.

The v2 export uses responsive AutoGrid rows. **Issues now** puts each provider's
incident title beside its affected services and impact, ordered by severity.
Unreported manual sources and collector failures appear in **All providers**,
so they do not bury customer incidents. Incident detail, history and coverage
rows are collapsed initially. Queries require the `source_catalog` view created
by the collector's migration 2; deploy the repaired collector before using them.

Migration and dashboard SQL regressions can be tested without a server using
an in-memory PGlite installation outside the repository:

```sh
npm install --prefix /tmp/skynet-migration-test --ignore-scripts @electric-sql/pglite@0.3.14
node scripts/check-migrations.mjs /tmp/skynet-migration-test/node_modules/@electric-sql/pglite/dist/index.js
```

The test covers fresh schemas, partial and complete manual schema changes,
preservation of provider data, and the two-Claude-incident display and recovery.
