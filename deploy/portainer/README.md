# Portainer helper stacks

These stacks run the optional feed and page-monitoring helpers used by
TVX-SKYNET. PostgreSQL and Grafana are deliberately not included because the
deployment uses the existing services.

The combinator stack builds the Rust application directly from the checked-out
Git repository in Portainer. It does not pull or publish an application image
from an external registry.

## One-time network setup

Create a bridge network named `tvx-skynet` in Portainer before deploying either
stack:

1. Open **Networks**.
2. Select **Add network**.
3. Use the name `tvx-skynet` and the `bridge` driver.

The future `external-status-combinator` stack will join the same external
network. Its internal helper URLs will then be:

```text
http://rss-bridge:80
http://changedetection:5000
```

## External status combinator

Deploy `combinator.yaml` as a **Git repository** stack in Portainer:

```text
Repository URL: https://github.com/archways404/TVX-SKYNET.git
Repository reference: refs/heads/main
Compose path: deploy/portainer/combinator.yaml
```

The relative build context in the stack requires repository deployment; do not
paste only the YAML into the web editor. Portainer checks out the repository
and builds the multi-stage `Dockerfile` locally on the Docker host.

Set this required stack environment variable, URL-encoding any special
characters in the password:

```text
DATABASE_URL=postgresql://external_status_writer:PASSWORD@192.168.1.221:5432/tvx_skynet?sslmode=disable
```

Optional variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `TVX_SKYNET_BIND_ADDRESS` | `0.0.0.0` | Host address on which the API is published. |
| `TVX_SKYNET_PORT` | `8080` | Published API port. |
| `RUST_LOG` | `external_status_combinator=info,tower_http=info` | Rust log filter. |

The container runs as UID `10001`, drops all Linux capabilities, uses a
read-only root filesystem, and runs the embedded PostgreSQL migrations before
starting the poll scheduler. The default source configuration is built into
the image.

After deployment, verify:

```text
http://PORTAINER-HOST:8080/health/live
http://PORTAINER-HOST:8080/health/ready
http://PORTAINER-HOST:8080/api/v1/status
```

## RSS-Bridge

Deploy `rss-bridge.yaml` as a Portainer stack. Supported stack environment
variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RSS_BRIDGE_BIND_ADDRESS` | `0.0.0.0` | Host address on which the UI port is published. |
| `RSS_BRIDGE_PORT` | `3005` | Published host port. |

The named volume `tvx-skynet-rss-bridge-config` is mounted at `/config` for a
future `config.ini.php`, bridge overrides, or a whitelist. RSS-Bridge must be
restarted after files in this volume are changed.

Do not expose an unrestricted RSS-Bridge instance directly to the public
Internet. Keep the host port on the trusted LAN or protect it at the reverse
proxy.

## changedetection.io

Deploy `changedetection.yaml` as a separate Portainer stack. It includes the
recommended Sockpuppet/Chrome browser so JavaScript-rendered status pages can
be monitored.

| Variable | Default | Purpose |
| --- | --- | --- |
| `CHANGEDETECTION_BASE_URL` | empty | Public URL used in links and notifications. Set this when reverse-proxied. |
| `CHANGEDETECTION_BIND_ADDRESS` | `0.0.0.0` | Host address on which the UI port is published. |
| `CHANGEDETECTION_PORT` | `5000` | Published host port. |
| `CHANGEDETECTION_FETCH_WORKERS` | `4` | Concurrent changedetection fetch workers. |
| `CHANGEDETECTION_BROWSER_WORKERS` | `4` | Maximum concurrent Chrome processes. |
| `CHANGEDETECTION_MINIMUM_RECHECK_SECONDS` | `60` | Lower bound for watch intervals. |
| `TZ` | `Europe/Stockholm` | Scheduler timezone. |

The Chrome endpoint is exposed only to containers on `tvx-skynet`; it is not
published on the host. The browser uses the upstream-recommended `SYS_ADMIN`
capability. If JavaScript page monitoring is not required, remove the browser
service, the `depends_on` block, and `PLAYWRIGHT_DRIVER_URL` to avoid that
capability and reduce memory use.

After the first login, configure changedetection's access password before
placing it behind a public reverse proxy. The future combinator API key should
be stored as a Portainer secret or environment variable, not committed to this
repository.

## Existing PostgreSQL connection

The future combinator container will use these non-secret settings:

```text
Host: 192.168.1.221
Port: 5432
Database: tvx_skynet
Schema: external_status
Username: external_status_writer
TLS mode: disable (only if the connection stays on the trusted LAN)
```

The password and complete `DATABASE_URL` belong in Portainer's environment or
secret management, not in a YAML file committed to Git.
