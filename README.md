# Flex

Flex is an open-source alternative to Plex: a **headless media server** that
indexes your video library and serves it over a JSON REST API, plus a **native
Linux desktop client** for browsing and playback.

Flex is deliberately small and dependency-light on the server side: point it at
one or more directories, let it extract metadata with `ffprobe`, store the index
in SQLite, and stream the original files to any client that speaks the API.

## Features

- **Library scanning** — walks configured directories and extracts container,
  video, audio and subtitle metadata with `ffprobe`.
- **SQLite index** — a single local database file; no external services.
- **JSON REST API** — libraries, paginated item browsing/filtering, scan
  triggers, and health checks (see [`docs/API.md`](docs/API.md)).
- **HTTP Range streaming** — the stream endpoint returns `206 Partial Content`
  so media players can seek efficiently.
- **Subtitle extraction** — embedded subtitle tracks are extracted on demand
  with `ffmpeg` and served as WebVTT; sidecar files are served directly.
- **Native desktop client** — GTK4 + libadwaita, with playback via `libmpv`.
  Browsing libraries/items and embedding mpv in the GTK window (via the X11
  window id, which works under XWayland), with an external `mpv` fallback on
  pure Wayland.

## Architecture

Flex is a single Cargo workspace with three crates:

| Crate                        | Role                                                             |
|------------------------------|------------------------------------------------------------------|
| `crates/flex-protocol`       | Shared serde wire types + constants (source of truth for the API).|
| `crates/flex-server`         | Headless scanner/indexer/streamer (Axum 0.8 + SQLite + ffprobe).  |
| `crates/flex-client`         | GTK4/libadwaita desktop player (libmpv).                          |

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the component diagram,
data flow, database schema and design decisions, and
[`docs/API.md`](docs/API.md) for the HTTP contract.

## Quick start

### 1. Build and run the server

```sh
# Configure: copy the example and edit the library paths
cp config/flex.example.toml flex.toml

# Run the server (scans on startup, then serves the API on :8096)
cargo run -p flex-server -- --config flex.toml

# Or trigger a scan explicitly on start, or register libraries on the CLI:
cargo run -p flex-server -- --config flex.toml --scan-on-start
cargo run -p flex-server -- --library "Movies=/srv/media/movies"
```

The server reads `FLEX_CONFIG` when `--config` is omitted and honours
`RUST_LOG` for log filtering (e.g. `RUST_LOG=info`). Default port is `8096`.

### 2. Launch the desktop client

```sh
cargo run -p flex-client -- --server http://127.0.0.1:8096
```

The client also reads the target server from the `FLEX_SERVER` environment
variable when `--server` is not passed:

```sh
FLEX_SERVER=http://media-host:8096 cargo run -p flex-client
```

## Requirements

- **Rust** stable (pinned in [`rust-toolchain.toml`](rust-toolchain.toml)),
  edition 2021.
- **ffmpeg / ffprobe** on `PATH` (metadata probing; on-demand subtitle
  extraction).
- **libsqlite3** development/runtime libraries (linked through `sqlx`).
- Server: nothing else — it is headless.
- Client: **GTK 4.6+**, **libadwaita 1.4+**, and **libmpv** (mpv 0.41 tested).

### Installing the system dependencies

```sh
# Debian / Ubuntu
sudo apt-get install ffmpeg pkg-config libsqlite3-dev \
    libgtk-4-dev libadwaita-1-dev libmpv-dev
```

## Configuration

Server configuration lives in a TOML file (`flex.toml` by default, overridable
with `--config` or the `FLEX_CONFIG` environment variable). See
[`config/flex.example.toml`](config/flex.example.toml) for every option,
including `host`, `port`, `database`, optional `cache_dir`, and one
`[[libraries]]` table per library.

```toml
host = "0.0.0.0"
port = 8096
database = "./data/flex.db"

[[libraries]]
name = "Movies"
path = "/srv/media/movies"
kind = "movies"

extensions = ["mkv", "mp4", "m4v", "avi", "mov", "webm"]
```

## API

The HTTP contract is documented in [`docs/API.md`](docs/API.md). The canonical
type definitions and route constants live in the `flex-protocol` crate, so the
server and client stay in sync by construction.

## Docker

A server-only container image is provided (the GUI client is excluded). See
[`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md) for full instructions.

```sh
docker build -t flex-server .
docker compose up -d
```

## Development

```sh
# Build the whole workspace
cargo build --workspace

# Run the test suite
cargo test --workspace

# Format and lint (both required before a PR)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings

# Convenience: copy a config if missing and run the server
./scripts/dev.sh

# End-to-end smoke test against a temporary server + sample media
./scripts/smoke.sh
```

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) for the full developer
workflow and pull-request expectations.

## Roadmap

The following are **not yet implemented** — they are planned directions:

- [ ] Authentication and multi-user accounts
- [ ] Transcoding / adaptive bitrate streaming
- [ ] Web UI
- [ ] Online subtitle search
- [ ] Watch state / resume / "continue watching"

## License

MIT — see [`LICENSE`](LICENSE).
