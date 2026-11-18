# Provider collection

All 42 default sources are collected automatically. No manual reports, tenant ID,
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

## Provider-map coverage and country navigation

The API includes live `metadata` for each provider (dependency, coverage, source
URL, domain and ISO country codes), even before its first successful poll. Memory
and PostgreSQL backends expose the same fields. Migration 4 adds country coverage;
it runs on startup. Country coverage describes a provider feed, not a nationwide outage.

LSYNC lists every returned provider, including carriers without current cases.
Country controls cover Sweden, Denmark, Norway, Finland and Belgium. Map pins are
not limited to Sweden. `network_details.locations[].geometry` optionally carries
WGS84 GeoJSON Polygon/MultiPolygon data alongside its approximate area centre.
Telia SE's polygons are converted from the provider's RT90DP projection; Telenor
SE's published bounding boxes are retained and shown with dashed boundaries.
GeoRSS points and feature names are preserved together with plain-text bulletin
updates. Invalid coordinates fail collection rather than producing misleading pins.

DNA Finland is a separate upstream-network source using the ArcGIS queries behind
https://kartat.dna.fi/Hairiokartta/ (layers 9, 10, 11 and 13). It covers voice, SMS,
ADSL broadband and mobile disturbances, excludes TV and maintenance layers, retains
municipality/postcode and provider polygons, and rejects partial/truncated query
responses. Its health does not establish Line Carrier platform/subscription health.

TDC's bulletin API supplies named Danish areas; the collector joins case IDs to the
`Drift_Markers` GeoJSON behind TDC NET's public mobile map. An anonymous map
session is obtained automatically. Published marker coordinates are retained;
markers that omit geometry stay in the case list. Closed/planned map layers are excluded.
Norlys supplies service bulletins. Norway's Erate/Telavox source publishes selected
country/service components; Proximus's public source covers Enco APIs, not Belgian
access-network faults. These cases remain visible without invented geographic pins.
Line Carrier currently publishes resolved bulletins without geographic coordinates;
its entry remains visible even when there are no active incidents.

The coordinated LSYNC snapshot contract can be checked without any deployed database:
`node scripts/check-lsync-contract.mjs <pglite-module-path> <TVX-LSYNC-checkout>`.
The test uses an embedded, in-memory database, runs the actual migrations and LSYNC
SQL, and verifies incidentless carriers, country metadata and polygon preservation.


## Dependency coverage reviewed 2026-11-18

The catalog has 64 sources: 45 automatic public collectors and 19 explicit gaps.
A `coverage_gap` entry deliberately fails collection with its configured reason.
It remains visible and unknown, with no successful poll or operational status.
This is an inventory of missing evidence, not an availability monitor; do not
interpret the source-health failure count as a vendor outage. Gaps poll hourly
without making network requests. Replace them with a verified collector once its
endpoint and health contract are available.

Added automatic collection:

- Simwood: official Statuspage `API and Portal` component, relevant to signup
  inventory. Does not test account inventory or reserve a number.
- Creditsafe: official public Hund HTML component states (`Connect API` and
  `Global Gateway Service and Integrated apps`). The Swedish GetData SOAP URL in
  `CreditChecker` has no dedicated public component, so this is advisory coverage.
  Unknown states remain unknown; missing/changed component markup fails collection.
- Nager.Date: read-only current-year PublicHolidays requests for FO, AD, GL and IS,
  the four countries used by `HolidayCalendarFetcherServiceImpl`. Each response
  must be nonempty with matching country/year and named holidays. Other countries
  use Enrico, a separate dependency; this check does not validate ten-year coverage,
  S3 writes or customer routing. Requests run every five minutes.

Billiant porting/NABS/outbox, Erate BSS/NRDB/CDR, Colt Apigee number management,
SNPAC/NUMPAC, GlobalConnect CDR delivery, Cention errands, UAE Pass, Etisalat
CloudTalk, Voxo, Telia/TDC MMS, Peppol/Visma, Nets and Printship now have explicit
entries describing the missing check. Authenticated API checks need dedicated
read-only accounts; SFTP checks need the configured host and a file-freshness
contract. Do not create orders, port numbers, send MMS, transcribe audio, print
invoices or submit payments to establish health. No credentials were added.
UAE Pass defaults to a staging issuer in source code: confirm its configured
production issuer rather than monitoring that default. Colt uses
`https://apis.colt.net/numberManagement`; the retired SOAP host is excluded.
Printship's host is configuration-driven, so no website/domain was guessed.
Google Cloud metadata now names the customer-facing Vertex AI/Gemini uses visible
in omni, chat and transcription code.

Telenor Norway is listed with the requested coverage-map reference. Its client
publishes raster fault exports and point-identify responses, not an enumerable
incident feed. Metadata/layer queries returned 403; the supported point identify
request returned 200. A browser check confirmed its frame embedding is blocked.
ORBYTE provides a direct official live-map link and translations for “Feil i
nettet” (Network faults), “Brudd” (Outage) and “Redusert kapasitet” (Reduced capacity).
Its raster areas are **not imported as ORBYTE dots**. It remains unknown in our
aggregate status. Norway's Telavox feed is separate and supplies no geographic
case coordinates. Do not fabricate Norway pins from a country name.

DNA Finland retains municipality/postcode and full polygons, including disjoint
areas and holes. Reports with `VIKA_LOPPUI` at or before observation time are now
excluded. Line Carrier bulletins without geographic coordinates stay in the list.

ORBYTE searches every published locality, folds Nordic accents (Malmö/Malmo,
Tromsø/Tromso), and provides a small built-in city gazetteer. A city search also
matches provider polygons containing that city, excluding holes. This is not a
worldwide geocoder or a guarantee of service at an address. Map bounds include
all polygon vertices; selection fits every area of a case. Wheel/keyboard zoom,
city labels and readable multiline tooltips are enabled. Known Danish/Norwegian
service terms have English labels; free-form reports keep their original text.

Validation on 2026-11-18: all 45 public collectors passed the explicit live check,
including Erate/Telavox Norway, DNA Finland and Line Carrier. DNA and Line Carrier
had no current geographic cases in that response; this does not test a specific
subscription. Unit fixtures cover Finnish polygons and resolved reports. Browser
verification used synthetic incidents for search, full-area display and wheel
zoom, not a simulated production outage.

GetAccept, Bring/Posten, BGC, Fastly, Intercom and Amplitude were not added from
name-only matches. GetAccept's continued use for BSS quote signing still needs
owner confirmation; it has not been represented as a verified dependency.
