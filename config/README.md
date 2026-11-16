# Source configuration

`sources.yaml` is built into `/app/config/sources.yaml`. Rebuild/redeploy the
Portainer repository stack after changing sources or collector code. The new
PostgreSQL migration runs at startup, before polling starts. Import the updated
Grafana JSON after this redeploy.

The configuration contains 41 providers: 27 public automatic sources, one
credential-dependent Microsoft source, and 13 manual sources. Status is
provider-reported health, not an end-to-end check of Telavox integrations.
`metadata` records the dependency, coverage limits, official page and logo domain;
PostgreSQL stores it in `external_status.source_catalog` for Grafana.

## Verified public sources (2026-11-16)

Every configured public source passed an actual collection with the Rust adapter.

| Collector | Sources / verified endpoint |
| --- | --- |
| Statuspage | Existing GitHub, Claude, Twilio, Jira Software, Sentry, Segment, HubSpot and ElevenLabs; new BankID, Teamleader, Upsales, SuperOffice, Stripe, Mapbox, Cloudflare, 1Password, Confluence and JSM |
| Google | [Workspace](https://www.google.com/appsstatus/dashboard/incidents.json), [Cloud](https://status.cloud.google.com/incidents.json), [Firebase](https://status.firebase.google.com/incidents.json) |
| Apple | [Developer JSONP](https://www.apple.com/support/systemstatus/data/developer/system_status_en_US.js), component `APNS` |
| Meta | [Public page data](https://metastatus.com/data/orgs.json), WhatsApp Business Platform services only |
| Salesforce | [Instance status preview](https://api.status.salesforce.com/v1/instances/status/preview), active production instances |
| Slack | [Current Status API v2](https://slack-status.com/api/v2.0.0/current) |
| RSS | [Azure public advisories](https://rssfeed.azure.status.microsoft/en-us/status/feed/), [S3 Frankfurt](https://status.aws.amazon.com/rss/s3-eu-central-1.rss) |

The Salesforce `/v1/instances/status` endpoint returned HTTP 400; the public
`/preview` endpoint returned instance states. Configure `include_components` with
exact Salesforce instance keys to avoid unrelated global incidents and maintenance.
It currently covers all active production instances, not customer-specific tenants.

Apple and Meta use undocumented page data, so schema changes fail collection or
show unknown. Meta and Salesforce expose component state here, without incident
narratives. Firebase is limited to `Cloud Messaging`; Cloudflare to `CDN/Cache`.
Google Workspace/Cloud status does not guarantee reCAPTCHA coverage. RSS has no
standard lifecycle: an advisory stays unknown unless its title explicitly marks it
resolved or it disappears from the feed; no age-based resolution is inferred.
Empty valid public feeds mean no reported incidents within that feed's coverage.

## Microsoft tenant health

The `microsoft_graph` collector uses OAuth client credentials and
`GET https://graph.microsoft.com/v1.0/admin/serviceAnnouncement/issues` with
`isResolved eq false`, follows pagination, and normalizes unresolved issues.
Set these Portainer environment variables:

- `MICROSOFT_TENANT_ID`
- `MICROSOFT_CLIENT_ID`
- `MICROSOFT_CLIENT_SECRET`

Register an application with the **application** permission
`ServiceHealth.Read.All` and grant administrator consent, as specified by
[Microsoft's API documentation](https://learn.microsoft.com/en-us/graph/api/serviceannouncement-list-issues?view=graph-rest-1.0).
Tokens are acquired for each poll; credentials stay in the environment. This
adapter has not been tested against a real tenant because credentials were not
provided. Missing credentials leave the row unknown with an explicit error.
The separate Azure public feed does not substitute for tenant service health.
Microsoft Dynamics incidents are included when Microsoft reports them for the tenant.

## Manual reports

Freja eID, Zendesk, Freshdesk/Freshworks, Pipedrive, Adyen, Spirius and seven carriers
(Telia, Telenor, TDC, Telia DK, Proximus, Line Carrier, Erate) use manual reports.
The dashboard explicitly labels these as manual; absent or expired reports remain
unknown. We did not confirm a reliable public JSON feed for these providers.
Adyen returned HTML at `/api/v2/summary.json`; Freshdesk redirected to HTML,
Freshworks' JSON request returned 403, Zendesk/Pipedrive's proposed JSON paths
returned 404, and Pipedrive's Statuspage host returned 401. Their public landing
pages were checked. No status URL is invented for Spirius or carriers.

The Portainer stack mounts a `manual_status` volume read-only at `/app/manual`.
The image seeds it with an empty `status.json`. Update that volume through an
administrative helper container or replace the volume mount with a host directory:

```yaml
volumes:
  - /your/status-directory:/app/manual:ro
```

Write `/your/status-directory/status.json` atomically. Use exact provider IDs from
`sources.yaml`, a UTC verification time, a normalized status and a nonempty note:

```json
{
  "telia": {
    "status": "degraded",
    "checked_at": "2026-11-16T10:00:00Z",
    "note": "Vendor confirmed intermittent call setup failures; ticket ABC-123"
  }
}
```

Reports expire after 24 hours (`stale_after` per source). Re-reading the file does
not extend their validity. Future timestamps and empty notes are rejected. Report
`operational` explicitly when the vendor confirms recovery. The poll reliability
panel records file-read attempts for these sources, not network probes. Missing
reports appear as failed reads in collector health, with an explanatory error.
For local runs, change manual `path` values to `config/manual-status.json` or an
appropriate local file; the shipped `/app/manual` path targets the container.

## Adding sources

A Statuspage collector takes the page base URL and appends
`api/v2/summary.json`. `include_components` matches names case-insensitively but
otherwise exactly; no matches fail collection. Without a filter, all non-group
components are included. Custom collectors use `url` endpoints. Unknown payload
values remain unknown and malformed responses fail collection.

Verify source-health and coverage in Grafana after a rebuild. Run tests with
`cargo test --locked`; optionally verify public endpoints with
`cargo test --locked live_public_sources -- --ignored --nocapture`.
