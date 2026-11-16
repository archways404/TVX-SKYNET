# Provider collection

All 41 default sources are collected automatically. No manual reports, tenant ID,
new database migration, browser container or changedetection stack is required.
Rebuild the existing combinator image after source or Rust changes.

37 sources publish usable health or incident information. Four carriers expose
public information pages without a verified global service-health signal. Those
pages are retrieved automatically, but their network status remains `unknown`.
A website returning HTTP 200 is never interpreted as an operational phone network.

## Newly automated sources

| Provider | Source | Coverage |
| --- | --- | --- |
| Telia SE | `https://glu2.han.telia.se/bios/glup` | Public fixed-network county and case HTML used by its map; broadband, IP telephony and TV. |
| Telenor SE | `https://mboss.telenor.se/outageportalfixed/` | Public fixed-network case JSON used by its map; anonymous page token and AJAX headers. |
| Microsoft 365 | `https://status.cloud.microsoft/api/posts/mac` | Public fallback advisories only. This is not tenant-specific Graph service health. |
| Freja eID | `https://status.signicat.com/api/v2/summary.json` | Signicat's `Freja eID` component is an explicitly identified upstream-provider proxy, not a direct Freja probe. |
| Zendesk | `https://status.zendesk.com/api/ssp/incidents.json` | Public incidents; the collector supplies the current date and excludes resolved/internal incidents. |
| Freshdesk | `https://public-api.freshstatus.io/v1/public-components/?account_id=3611` | Regional components. Provider-marked `ignore_overall_status` entries are excluded; region names remain visible. |
| Pipedrive | `https://status.pipedrive.com/` | Component states and current incident titles parsed from the public HTML. |
| Adyen | `https://status.adyen.com/api/incident-messages/active` | Public active incidents. |
| Spirius | `https://spirius.instatus.com/v3/summary.json` | Public page state and active incident details when provided. |
| Line Carrier | `https://www.linecarrier.fi/hairiotiedotteet/feed/` | Provider RSS. Titles beginning `Korjattu:` are resolved; unclassified advisories stay unknown. |

Microsoft, Freshdesk, Zendesk, Adyen and Pipedrive use the data consumed by their
public pages. These formats can change; malformed payloads fail collection rather
than becoming green. Freja's third-party proxy coverage is stated in the dashboard.

Google Workspace, Google Cloud, Firebase/FCM, Apple APNs, Meta/WhatsApp,
Salesforce, Slack, Azure and AWS S3 keep their existing public adapters. Other
providers use the Statuspage adapter. Salesforce currently covers global active
production instances; narrow `include_components` when instance scope is known.
Google status feeds do not guarantee reCAPTCHA coverage. Meta and Salesforce
currently provide component states without full incident narratives.

## Carrier coverage limits

Telia SE and Telenor SE now collect actual cases from their official public
fixed-network maps, including affected services, county viewport associations,
start times and estimated repair times. Telia also reports affected customer
counts. Cases are deduplicated across viewports. These collectors cover broadband,
IP telephony and TV, not mobile networks or an end-to-end Telavox connection.
A local case produces a partial outage, not a claim of a nationwide network failure.

TDC, Telia DK/Norlys, Proximus and Erate still have `public_notice` checks.
Their pages are fetched automatically but do not yet produce verified outage data.
They appear as **Public info only**, remain unknown, and count as coverage gaps.
These four collectors need further source investigation; they are not complete
outage monitoring. Neither a successful fetch nor arbitrary page changes imply
healthy service or an outage.

## Optional authenticated / manual collectors

The older `microsoft_graph` and `manual` collector types remain available for
custom configurations; none is used by the default sources. Graph requires an
app registration with application permission `ServiceHealth.Read.All`, administrator
consent, and `MICROSOFT_TENANT_ID`, `MICROSOFT_CLIENT_ID`, `MICROSOFT_CLIENT_SECRET`.
Public Microsoft collection needs none of those variables.

## Configuration and deployment

`metadata` defines the short dependency description, coverage limit, status link
and icon domain. PostgreSQL stores these on `external_status.sources`.
The dashboard queries that existing table, avoiding a separate view permission.

Statuspage `include_components` matches names case-insensitively but exactly.
No matches fail collection. Without a filter all non-group components are tracked.

1. Merge changes into `archways404/TVX-SKYNET` main.
2. Pull and redeploy `deploy/portainer/combinator.yaml` from that repository,
   rebuilding the image.
3. Confirm `/health/ready` and `/api/v1/source-health`.
4. Apply the regenerated `deploy/grafana/tvx-skynet-dashboard-v2.json` and save.

Run `cargo test --locked` for regressions. To verify the configured public sources:
`cargo test --locked live_public_sources -- --ignored --nocapture`.
