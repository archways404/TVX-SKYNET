# TVX-SKYNET

TVX-SKYNET is an external-service status combinator. It polls structured provider status sources, normalizes their component and incident states, and exposes one read-only API for Grafana and internal applications.

The current build is the first vertical slice:

- Rust/Tokio polling engine with bounded concurrency and per-source timeouts.
- Reusable Atlassian Statuspage collector.
- GitHub and Claude/Claude Code sources.
- Component filtering and normalized service/incident states.
- In-memory and PostgreSQL status stores behind one storage trait.
- Read-only Axum API and health endpoints.
- Stale-source and failed-poll tracking.
- Portainer stacks for RSS-Bridge and changedetection.io helper services.

The PostgreSQL store includes durable incident/update history, provider and
component observations, poll-attempt history, and read-only Grafana views. The
combinator container and Grafana provisioning are the next deployment slice.

## Run

```powershell
cargo run
```

The default configuration is [`config/sources.yaml`](config/sources.yaml). Set `EXTERNAL_STATUS_CONFIG` to use a different file.

The server listens on `0.0.0.0:8080` by default.

## API

```text
GET /api/v1/status
GET /api/v1/providers
GET /api/v1/providers/{id}
GET /api/v1/incidents
GET /api/v1/source-health
GET /health/live
GET /health/ready
```

The scheduler polls immediately at startup, then follows each source's configured interval. A failed poll preserves the last successful provider state; the source becomes `unknown` after its stale threshold instead of being reported as operational.

## Storage

The default backend is in-memory storage:

```text
STORAGE_BACKEND=memory
```

To use PostgreSQL:

```text
STORAGE_BACKEND=postgres
DATABASE_URL=postgresql://external_status_writer:PASSWORD@192.168.1.221:5432/tvx_skynet?sslmode=disable
```

The `external_status` schema must already exist and be owned by, or grant
`CREATE` to, `external_status_writer`. Embedded SQLx migrations run at startup
before polling begins. Secrets belong in Portainer; `.env.example` contains
only the expected variable shape.

The migration creates current-state tables, append-only observation and poll
history, incident/update history, and the Grafana-oriented views
`external_status.provider_current` and `external_status.active_incidents`.

## Portainer helpers

Portainer-ready stacks for the combinator, RSS-Bridge and changedetection.io,
along with environment options and the shared-network setup, are documented in
[`deploy/portainer/README.md`](deploy/portainer/README.md).

## Verify

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Next vertical slices

1. Exercise the embedded migrations through the Portainer deployment.
2. Add the generic RSS/Atom/JSON Feed collector using `feed-rs`.
3. Add conditional requests, jitter and retry handling.
4. Provision the Grafana datasource and dashboard against read-only database views.

