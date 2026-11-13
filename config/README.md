# Source configuration

`sources.yaml` is compiled into the production container at
`/app/config/sources.yaml`. A source change therefore requires rebuilding the
Portainer repository stack.

## Add an Atlassian Statuspage source

Use the public page base URL, not the `/api/v2/summary.json` endpoint. The
collector appends that path itself.

```yaml
- id: provider-id
  name: Provider name
  category: category-name
  collector:
    type: statuspage
    base_url: https://status.example.com/
  include_components:
    - Exact component name
    - Another exact component name
```

Component matching is case-insensitive but otherwise exact. If
`include_components` is omitted, all non-group components are tracked. The
provider's effective state is the worst selected component state. If the page
does not expose components, the page-level status indicator is used.

After changing the file:

1. Commit and push the repository change.
2. Open the `tvx-skynet-combinator` stack in Portainer.
3. Pull/redeploy the repository stack so Portainer rebuilds the local image.
4. Check `/api/v1/source-health` on the published combinator port.
5. Confirm the new provider appears in Grafana after the first successful poll.

## Current direct sources

- GitHub
- Claude and Claude Code
- Twilio Network Traversal Service and REST API
- Jira
- Sentry
- Segment
- HubSpot
- ElevenLabs page-level status

## Sources needing another collector

- RSS/Atom: AWS and other providers with official feeds.
- Custom JSON: Apple Developer and Google/Firebase dashboards.
- changedetection.io: Microsoft 365 and public pages without a stable feed or
  unauthenticated status API.
- Carrier and porting providers: usually vendor notifications, authenticated
  portals, email ingestion, or an internal manual source.

RSS-Bridge output will use the RSS/Atom collector once that collector is
implemented. changedetection.io will be integrated through its API rather than
scraping its user interface.
