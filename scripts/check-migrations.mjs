// Run with an installed @electric-sql/pglite module, or supply its module path:
// node scripts/check-migrations.mjs /tmp/skynet-migration-test/node_modules/@electric-sql/pglite/dist/index.js
// Every database is embedded and in memory. No DATABASE_URL is read.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const { PGlite } = await import(process.argv[2] ?? '@electric-sql/pglite');
const root = new URL('../', import.meta.url);
const initial = await readFile(new URL('migrations/0001_external_status.sql', root), 'utf8');
const coverage = await readFile(new URL('migrations/0002_source_coverage.sql', root), 'utf8');
const dashboard = JSON.parse(await readFile(new URL('deploy/grafana/tvx-skynet-dashboard-v2.json', root), 'utf8'));
for (const scenario of ['fresh', 'partial-manual', 'complete-manual']) {
  const db = new PGlite();
  try {
    await db.exec('CREATE SCHEMA external_status');
    await db.exec(initial);
    await db.query("INSERT INTO external_status.sources (id, name, category, stale_after_seconds) VALUES ('telia', 'Telia', 'carriers', 86400)");
    if (scenario === 'partial-manual') {
      await db.exec("ALTER TABLE external_status.sources ADD COLUMN dependency TEXT NOT NULL DEFAULT ''");
      await db.query("UPDATE external_status.sources SET dependency = 'Existing customer calling data'");
    }
    if (scenario === 'complete-manual') {
      // Simulate the user running the schema SQL by hand before SQLx startup.
      await db.exec(coverage);
      await db.query("UPDATE external_status.sources SET dependency = 'Existing customer calling data'");
    }
    await db.exec(coverage);
    const { rows } = await db.query("SELECT id, dependency, collection_mode FROM external_status.source_catalog WHERE id = 'telia'");
    assert.equal(rows.length, 1, 'existing provider must survive');
    if (scenario !== 'fresh') assert.equal(rows[0].dependency, 'Existing customer calling data');
    assert.equal(rows[0].collection_mode, 'automatic');
    // Reproduce the deployed Grafana role: old tables are readable, but a new
    // view created by migration 2 has no implicit SELECT grant.
    await db.exec(`CREATE ROLE dashboard_reader;
      GRANT USAGE ON SCHEMA external_status TO dashboard_reader;
      GRANT SELECT ON external_status.sources, external_status.provider_current,
        external_status.active_incidents, external_status.components,
        external_status.poll_attempts, external_status.provider_observations TO dashboard_reader;
      SET ROLE dashboard_reader;`);
    for (const panel of Object.values(dashboard.spec.elements)) {
      for (const query of panel.spec.data.spec.queries) {
        const sql = query.spec.query.spec.rawSql
          .replaceAll(/\$__timeFilter\([^)]*\)/g, 'TRUE')
          .replaceAll('${provider:regex}', '.*');
        await db.query(sql);
      }
    }
    await db.exec('RESET ROLE');
    if (scenario === 'fresh') {
      await db.exec(`
        INSERT INTO external_status.sources (id, name, category, stale_after_seconds, current_status, last_success_at)
        VALUES ('claude', 'Claude', 'developer-tools', 900, 'partial_outage', now());
        INSERT INTO external_status.components (provider_id, external_id, name, current_status, provider_updated_at, last_seen_at)
        VALUES ('claude', 'ios', 'Claude iOS app', 'partial_outage', now(), now()),
               ('claude', 'api', 'Claude API', 'degraded', now(), now());
        INSERT INTO external_status.incidents (provider_id, external_id, title, lifecycle, impact, started_at, provider_updated_at, last_seen_at, affected_component_ids)
        VALUES ('claude', 'ios-signup', 'Some users unable to sign up with Apple on the Claude iOS app', 'investigating', 'partial_outage', now(), now(), now(), ARRAY['ios']),
               ('claude', 'model-errors', 'Elevated errors for Claude Sonnet 5.5 and Claude Fable 5.2', 'monitoring', 'degraded', now(), now(), now(), ARRAY['api']);
      `);
      await db.exec("UPDATE external_status.sources SET current_status = 'operational' WHERE id = 'claude'");
      const summarySql = dashboard.spec.elements['panel-1'].spec.data.spec.queries[0].spec.query.spec.rawSql;
      const summary = await db.query(summarySql);
      assert.equal(Object.values(summary.rows[0])[0], 4, 'an active partial-outage incident must affect overall health even when components are green');
      const sql = dashboard.spec.elements['panel-2'].spec.data.spec.queries[0].spec.query.spec.rawSql;
      const { rows: issues } = await db.query(sql);
      assert.equal(issues.length, 2, 'show both Claude incidents; keep unreported manual sources out of the incident list');
      assert.deepEqual(issues.map(i => i['Issue']), [
        'Some users unable to sign up with Apple on the Claude iOS app',
        'Elevated errors for Claude Sonnet 5.5 and Claude Fable 5.2',
      ]);
      assert.deepEqual(issues.map(i => i['Affected service']), ['Claude iOS app', 'Claude API']);
      await db.exec("UPDATE external_status.incidents SET lifecycle = 'resolved', resolved_at = now() WHERE provider_id = 'claude'");
      await db.exec("UPDATE external_status.sources SET current_status = 'operational' WHERE id = 'claude'");
      const cleared = await db.query(sql);
      assert.equal(cleared.rows.length, 1);
      assert.equal(cleared.rows[0]['Issue'], 'No reported service incidents');
    }
    console.log(`${scenario}: migration and all dashboard SQL queries pass; existing data preserved`);
  } catch (error) {
    console.error(`${scenario}: ${error.message}`);
    process.exitCode = 1;
    break;
  } finally {
    await db.close();
  }
}
