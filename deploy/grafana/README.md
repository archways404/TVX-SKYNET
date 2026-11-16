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

## Expanded provider coverage and Grafana v2

Use **[tvx-skynet-dashboard-v2.json](tvx-skynet-dashboard-v2.json)** for the supplied
Grafana v2 installation. It preserves dashboard identity
`tvx-skynet-external-status` and PostgreSQL datasource UID `dg46qijl86fi8d`.
The original `tvx-skynet-dashboard.json` remains a v1 import with datasource mapping.
Both exports include the same 11 panels: the existing eight plus a customer-critical
summary, a dependency coverage table, and a reading guide. Provider metadata drives
logos, categories and coverage, so new rows need no hard-coded dashboard edits.

Rebuild/redeploy the collector **before** importing either updated export: the
queries require migration `0002_source_coverage.sql` and new source initialization.
The dashboard queries stored PostgreSQL observations; importing JSON alone does
not add collection. Unknown manual and unauthenticated Microsoft rows are expected
until configured. See [source setup](../../config/README.md) for Microsoft app
credentials, manual reports and provider coverage limits.

To regenerate the v2 export after editing the maintained v1 file:

```sh
python3 scripts/build-dashboard-v2.py
```

Provider selection filters component and poll-detail panels. Overview summaries
continue to include all enabled providers. Timeline rows show successful observations;
check current freshness alongside history. Salesforce currently covers global
production instances: narrow its configured instance keys when your customer scope
is known. No live Grafana import or PostgreSQL migration was performed locally.


## Responsive layout and migration recovery

The v2 export uses responsive AutoGrid rows. **Issues now** lists each provider,
affected service and exact incident title, ordered by severity. Missing manual
reports and collection failures remain visible in **All providers**. Detailed
incident, history and coverage sections start collapsed.

The generator converts classic null base thresholds to numeric zero because this
installation's Grafana code editor rejects null threshold values. Validate with:

```sh
python3 scripts/build-dashboard-v2.py
python3 scripts/check-dashboard.py
```

Migration 2 can be run after its SQL has been applied manually: existing columns
are retained and the coverage view is created or replaced. Rebuild/redeploy the
collector image to load that repaired embedded migration; restarting the old
container retains the original SQL. Do not delete existing provider data.

For isolated PostgreSQL regression checks (no production connection):

```sh
npm install --prefix /tmp/skynet-migration-test --ignore-scripts @electric-sql/pglite@0.3.14
node scripts/check-migrations.mjs /tmp/skynet-migration-test/node_modules/@electric-sql/pglite/dist/index.js
```

The regression check covers fresh, partially manual and fully manual schema
setups, preserves existing values, executes dashboard SQL, and verifies both
Claude example incidents appear with their affected services.
