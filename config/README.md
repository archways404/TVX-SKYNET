# Provider collection

All 41 default sources are collected automatically. No manual reports, tenant ID,
browser container or changedetection stack is required.
Rebuild the existing combinator image after source or Rust changes.

All default sources now parse actual provider status, incident or service-bulletin
data; no default source is a homepage-only `public_notice` check. Coverage remains
scoped: a successful public feed does not prove every customer connection works.

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

## Carrier sources and scope

| Provider | Automatic collection | Scope |
| --- | --- | --- |
| Telia SE | Public map county/case HTML | Broadband and IP telephony (TV excluded); case IDs, viewport-associated counties, timestamps and customer counts. |
| Telenor SE | Public map JSON using its anonymous token and AJAX headers | Broadband/telephony cases (TV excluded), provider area coordinates, start and estimated repair times. |
| TDC / TDC NET | `https://tdc.dk/api/driftsinfo/getfasdata` | Public mobile, broadband/IP, access and transport incidents. Explicit resolution and planned windows determine current state. |
| Telia DK / Norlys | Official current-bulletin HTML | DataNet, fixed, mobile and UC (hosting excluded). Bulletins more than 24 hours beyond their repair estimate, or lacking one, remain unconfirmed/unknown. Planned-work sections are excluded. |
| Proximus | `https://status.enco.io/api/v2/summary.json` | **Enco voice/SMS/fixed-mobile-unification APIs only.** This does not establish Belgian access-network or wholesale trunk health. |
| Line Carrier | Official Finnish RSS | Resolved `Korjattu:` titles excluded. Other public advisories preserve uncertain impact. |
| Erate / Telavox Norway | `https://status.io/1.0/status/5c77e330cac2ed049a438709` | Erate's website redirects to Telavox Norway and links its official status page. Only Norway components/incidents are collected; no independent Erate MVNO/wholesale feed was verified. |

Carrier incidents remain separate from service-provider incidents in Grafana.
Every network case has its own row; the Network provider selector narrows both
carrier panels. Regional faults do not mean the carrier is unavailable nationwide.
Proximus wholesale/access and independent Erate platform coverage remain limited;
these limits are not solved merely by polling the public API more frequently.

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

## Network incident presentation

TV/IPTV is disabled in Telia and Telenor collection. TDC's service allowlist already
excludes TV/radio and energy. Norlys excludes hosting. Removed cases are resolved
and removed components become inactive on the next successful poll after deployment.

The network table separates Location, Affected service and Issue. Telia includes
provider area codes and links to the actual polygon viewport. Telenor links to
the centre of its published bounding box and labels it approximate; it does not
invent an address or a fault cause. TDC uses its published location title and
region separately from the reported reason/consequence. Migration 3 stores structured incident location data and adds it to the existing active-incidents view. It runs automatically on collector startup.

The built-in Grafana Geomap displays only reports with published coordinates. Telia
RT90DP coordinates are transformed to WGS84 using the provider's exact projection
definition; Telia and Telenor markers show approximate area centres, not addresses.
Overlapping reports are grouped by provider and position. Reports without coordinates
remain in the table without guessed pins. No PostGIS or geocoding service is required.
