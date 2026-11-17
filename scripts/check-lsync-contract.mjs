// Embedded database only. Usage: node scripts/check-lsync-contract.mjs <pglite-module> <LSYNC-checkout>
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
const { PGlite } = await import(process.argv[2] ?? '@electric-sql/pglite')
const lsync = process.argv[3]
if (!lsync) throw new Error('Supply the coordinated TVX-LSYNC checkout')
const db = new PGlite()
try {
  await db.exec('CREATE SCHEMA external_status')
  for (const file of ['0001_external_status.sql', '0002_source_coverage.sql', '0003_network_locations.sql', '0004_provider_countries.sql']) {
    await db.exec(await readFile(new URL(`../migrations/${file}`, import.meta.url), 'utf8'))
  }
  await db.exec(`INSERT INTO external_status.sources (id,name,category,stale_after_seconds,country_codes,coverage,status_url)
    VALUES ('line-carrier','Line Carrier','carriers',900,ARRAY['FI'],'Finnish bulletins','https://www.linecarrier.fi/hairiotiedotteet/'),
    ('tdc','TDC','carriers',900,ARRAY['DK'],'Danish provider map','https://daekning.tdc.dk/');`)
  const details = {location: 'Aarhus', services: ['Mobile voice and data'], locations: [{name: 'Aarhus',latitude:56.1,longitude:10.2,precision:'provider_bounds',geometry:{type:'Polygon',coordinates:[[[10,56],[11,56],[11,57],[10,56]]]}}]}
  await db.query(`INSERT INTO external_status.incidents (provider_id,external_id,title,lifecycle,impact,started_at,provider_updated_at,last_seen_at,network_details)
    VALUES ('tdc','case-1','Mobile fault','investigating','partial_outage',now(),now(),now(),$1::jsonb)`, [JSON.stringify(details)])
  const source = await readFile(path.join(lsync, 'server/src/store/external_status.rs'), 'utf8')
  const sql = source.split('const SNAPSHOT: &str = r#"')[1].split('"#;')[0]
  const { rows } = await db.query(sql)
  const snapshot = Object.values(rows[0])[0]
  assert.equal(snapshot.providers.length, 2)
  const carrier = snapshot.providers.find((p) => p.id === 'line-carrier')
  assert.equal(carrier.stale, true)
  assert.deepEqual(carrier.metadata.country_codes, ['FI'])
  assert.deepEqual(carrier.active_incidents, [])
  const tdc = snapshot.providers.find((p) => p.id === 'tdc')
  assert.deepEqual(tdc.active_incidents[0].network_details, details)
  console.log('LSYNC database snapshot preserves provider visibility, country metadata and complete map geometry')
} finally { await db.close() }
