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

### Windows

For Windows console and Windows Service installation, see [`docs/WINDOWS.md`](WINDOWS.md).

### Linux (systemd)

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

To bootstrap the first admin in the container, pass the bootstrap environment
variables once (create the account on the first run, then remove them):

```sh
docker run --rm -p 8096:8096 \
  -e FLEX_ADMIN_USER=admin \
  -e FLEX_ADMIN_PASSWORD='choose-a-strong-password' \
  -v /srv/media:/media:ro \
  -v flex-data:/data \
  -v /etc/flex/flex.toml:/etc/flex/flex.toml:ro \
  flex-server
```

### Hardware transcoding (NVIDIA)

`ffmpeg` is installed in the image, so **software** remux/transcode with
`libx264` works out of the box. Hardware (NVENC) acceleration is **not enabled
by default** — the image starts CPU-only and requests no GPU device. To use an
NVIDIA GPU you must pass it through explicitly with the NVIDIA Container
Toolkit:

```sh
docker run --gpus all ... flex-server      # requires nvidia-container-toolkit
```

NVENC is used only when the GPU is visible to the process. On an RTX 2070 SUPER
(Turing) this means `h264_nvenc`/`hevc_nvenc`; AV1 encode is unavailable on
Turing. See [`TRANSCODING.md`](TRANSCODING.md).

### Docker Compose

[`docker-compose.yml`](../docker-compose.yml) wires up the common case: the
media directory mounted read-only, a data directory for the SQLite database,
the port published, and `RUST_LOG=info`. It documents the
`FLEX_ADMIN_USER`/`FLEX_ADMIN_PASSWORD` bootstrap variables (commented out) so
you can create the first admin on an unattended first run.

```sh
docker compose up -d
docker compose logs -f flex-server
```

Edit the compose file to point the media volume at your library path and, if
desired, uncomment the config bind mount. Inside the container your config
should reference the mounted paths, for example:

```toml
# When behind a reverse proxy on the host, use a private/loopback bind and
# route to the container port. Publishing 8096 publicly is not recommended.
host = "0.0.0.0"
port = 8096
database = "/data/flex.db"

[[libraries]]
name = "Movies"
path = "/media/movies"
kind = "movies"
```

### Volumes and ports

| Mount / port / env              | Purpose                                                        |
|---------------------------------|----------------------------------------------------------------|
| `./media:/media:ro`             | Your media library, mounted **read-only**.                     |
| `./data:/data`                  | SQLite database + optional transcode cache. Persist this across restarts. |
| `127.0.0.1:8096:8096`           | Server port, bound to loopback by default (put the TLS proxy in front). |
| `FLEX_ADMIN_USER/PASSWORD`      | Optional one-time first-run admin bootstrap.                   |

The database file must live on a persistent volume; if it is inside the
container's writable layer it is lost when the container is replaced.

## Create the first admin

Flex requires authentication for every route except `GET /health` and
`POST /auth/login`, and there is no self-service signup. Create the initial
admin from the CLI before (or after) starting the server, using the same config:

```sh
sudo -u flex flex-server --config /etc/flex/flex.toml \
  user add admin --admin --password 'choose-a-strong-password'

# Other account management:
sudo -u flex flex-server --config /etc/flex/flex.toml user list
sudo -u flex flex-server --config /etc/flex/flex.toml user passwd admin
sudo -u flex flex-server --config /etc/flex/flex.toml user delete <name>
```

The password can also come from `$FLEX_PASSWORD` or an interactive prompt if
`--password` is omitted. Note that the interactive prompt does **not** suppress
echo, so prefer `--password`/`$FLEX_PASSWORD` or a shell that controls the
terminal.

For unattended provisioning (e.g. a container), set `FLEX_ADMIN_USER` and
`FLEX_ADMIN_PASSWORD` in the environment before the server starts; when the
database has **no users yet**, the admin is created on first run from those
values. **Unset them after the first run** — leaving bootstrap credentials in
the environment is a standing risk. See [`SECURITY.md`](SECURITY.md).

## Securing with a reverse proxy

Flex speaks plain HTTP and has no built-in TLS. For anything reachable outside a
trusted LAN, terminate TLS at a reverse proxy (Caddy or nginx) and bind Flex to
loopback:

```toml
# flex.toml
host = "127.0.0.1"
port = 8096
```

```
Internet ──TLS(443)──▶ Caddy/nginx ──HTTP(127.0.0.1:8096)──▶ flex-server
```

- Do **not** port-forward 8096. Expose only 80/443 publicly.
- Preserve `Authorization` and `Range` headers, disable response buffering, and
  do **not** gzip media.
- Full rationale: [`SECURITY.md`](SECURITY.md). Copy-pasteable Caddy and nginx
  configurations, DNS/Let's Encrypt notes, and a curl verification recipe:
  [`REVERSE_PROXY.md`](REVERSE_PROXY.md).

### Run behind Caddy

Caddy obtains and renews Let's Encrypt certificates automatically. With an
`A`/`AAAA` record for `media.example.com` pointing at the host and ports 80/443
open:

```caddyfile
media.example.com {
	reverse_proxy 127.0.0.1:8096 {
		flush_interval -1
	}
}
```

Reload with `sudo systemctl reload caddy`. See
[`REVERSE_PROXY.md`](REVERSE_PROXY.md) for the nginx alternative and streaming
tuning.

## Networking and firewall

- Expose Flex **only through the TLS reverse proxy**; bind `host = "127.0.0.1"`
  (or a private interface firewalled to the proxy).
- Authentication is enforced on every route except `/health` and `/auth/login`,
  and logins are rate limited — but that is not a reason to expose the app
  port directly.
- Keep 8096 unreachable from the public internet.

```sh
# ufw example: allow the proxy to reach Flex, keep 8096 private
sudo ufw allow 80,443/tcp
sudo ufw deny 8096/tcp
```

## Point the client at the server

The desktop client takes the server URL on the command line or via
`FLEX_SERVER`. Point it at the TLS reverse proxy for remote use, or the
loopback/LAN URL locally:

```sh
flex-client --server https://media.example.com
# or
FLEX_SERVER=https://media.example.com flex-client
```

## Verifying a deployment

```sh
# Health is public.
curl https://media.example.com/api/v1/health
# {"status":"ok","version":"0.1.0","api_version":"v1"}

# Everything else needs a bearer token. Log in first:
TOKEN=$(curl -sS -X POST https://media.example.com/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"YOUR_PASSWORD","device_name":"curl"}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')

# Authenticated requests:
curl -H "Authorization: Bearer ${TOKEN}" https://media.example.com/api/v1/items
curl -H "Authorization: Bearer ${TOKEN}" -X POST https://media.example.com/api/v1/scan
curl -H "Authorization: Bearer ${TOKEN}" https://media.example.com/api/v1/system/info
```

For a self-contained local test that creates sample media, an admin, logs in and
exercises the scan, item listing and a Range request with a bearer token, run
[`scripts/smoke.sh`](../scripts/smoke.sh).
