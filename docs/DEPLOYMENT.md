# Deploying Flex

This guide covers running the **server** in production: a release build, a
systemd service, and a Docker container. The GUI client is a desktop
application and is not part of the server image.

## Build a release binary

```sh
cargo build --release -p flex-server
```

The binary lands at `target/release/flex-server`. Copy it somewhere on `PATH`
(e.g. `/usr/local/bin/flex-server`) for the systemd unit below.

```sh
sudo install -m 0755 target/release/flex-server /usr/local/bin/flex-server
```

## Run as a systemd service

A hardened example unit is provided at
[`deploy/flex-server.service`](../deploy/flex-server.service).

1. Create the service user and its state directory:

   ```sh
   sudo useradd --system --home /var/lib/flex --shell /usr/sbin/nologin flex
   sudo install -d -o flex -g flex /var/lib/flex /etc/flex
   ```

2. Copy the config and edit it (`host`, `port`, `database`, `[[libraries]]`).
   With `WorkingDirectory=/var/lib/flex`, a relative database path such as
   `./data/flex.db` resolves under `/var/lib/flex`. An absolute path such as
   `/var/lib/flex/flex.db` is recommended for clarity.

   ```sh
   sudo cp config/flex.example.toml /etc/flex/flex.toml
   sudo editor /etc/flex/flex.toml
   ```

   The unit starts the server with `--config /etc/flex/flex.toml`; the database
   and optional cache live alongside the service state under `/var/lib/flex`.
   Make sure the `flex` user can **read** the media directories and **write**
   the database directory.

3. Install and start the unit:

   ```sh
   sudo cp deploy/flex-server.service /etc/systemd/system/flex-server.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now flex-server
   sudo systemctl status flex-server
   ```

4. Follow the logs:

   ```sh
   journalctl -u flex-server -f
   ```

The example unit sets `RUST_LOG=info` and includes basic hardening
(`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`,
`ReadWritePaths=/var/lib/flex`). If your media lives under `/home`, adjust
`ProtectHome` accordingly (e.g. `ProtectHome=read-only` plus an explicit
`ReadOnlyPaths=/home/media`).

## Docker

The [`Dockerfile`](../Dockerfile) builds the server only and runs it on
`debian:bookworm-slim` with `ffmpeg` and `libsqlite3`.

```sh
docker build -t flex-server .
docker run --rm -p 8096:8096 \
  -v /srv/media:/media:ro \
  -v flex-data:/data \
  -v /etc/flex/flex.toml:/etc/flex/flex.toml:ro \
  flex-server
```

The image's default entrypoint is `/usr/local/bin/flex-server` with
`CMD ["--config","/etc/flex/flex.toml"]`.

### Docker Compose

[`docker-compose.yml`](../docker-compose.yml) wires up the common case: the
media directory mounted read-only, a data directory for the SQLite database,
the port published, and `RUST_LOG=info`.

```sh
docker compose up -d
docker compose logs -f flex-server
```

Edit the compose file to point the media volume at your library path and, if
desired, uncomment the config bind mount. Inside the container your config
should reference the mounted paths, for example:

```toml
host = "0.0.0.0"
port = 8096
database = "/data/flex.db"

[[libraries]]
name = "Movies"
path = "/media/movies"
kind = "movies"
```

### Volumes and ports

| Mount / port        | Purpose                                                        |
|---------------------|----------------------------------------------------------------|
| `./media:/media:ro` | Your media library, mounted **read-only**.                     |
| `flex-data:/data`   | SQLite database + optional cache. Persist this across restarts.|
| `8096:8096`         | Default server port.                                           |

The database file must live on a persistent volume; if it is inside the
container's writable layer it is lost when the container is replaced.

## Networking and firewall

- Flex has **no authentication yet**. Run it only on a trusted LAN or behind a
  VPN, and **do not port-forward it to the public internet**.
- To expose it on the LAN, bind `host = "0.0.0.0"` (the example default) and
  allow the port through the host firewall.
- To restrict it to the local machine, bind `host = "127.0.0.1"`.

```sh
# ufw example: allow only the local subnet
sudo ufw allow from 192.168.0.0/24 to any port 8096 proto tcp
```

## Point the client at the server

The desktop client takes the server URL on the command line or via
`FLEX_SERVER`:

```sh
flex-client --server http://media-host:8096
# or
FLEX_SERVER=http://media-host:8096 flex-client
```

## Verifying a deployment

```sh
curl http://media-host:8096/api/v1/health
# {"status":"ok","version":"0.1.0","api_version":"v1"}

curl -X POST http://media-host:8096/api/v1/scan
curl http://media-host:8096/api/v1/items
```

For a self-contained local test that creates sample media and exercises the
scan, item listing and a Range request, run [`scripts/smoke.sh`](../scripts/smoke.sh).
