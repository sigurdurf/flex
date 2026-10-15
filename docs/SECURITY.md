# Flex Security

This document describes the threat model for a self-hosted Flex server that is
reachable from the public internet through a TLS reverse proxy, and the
controls Flex provides. It is written for operators exposing
`media.example.com` (Caddy/nginx + Let's Encrypt) rather than a LAN- or
VPN-only deployment.

If you only ever run Flex on a trusted LAN, read the
[hardening checklist](#hardening-checklist) anyway — the defaults are the same,
and a compromised home network is still a compromised media server.

## Threat model

Flex is a **single-user-process application server** that:

- holds a database of your media metadata and user accounts,
- reads media/artwork/subtitle files from disk,
- runs `ffmpeg`/`ffprobe` subprocesses,
- listens on one TCP port.

The assets worth protecting are:

| Asset | Why it matters |
|-------|----------------|
| Account passwords | Reused elsewhere; a leak is a credential-stuffing supply. |
| Device tokens | Bearer credentials; possession grants full API access as that user. |
| Media library files | Private content; path-traversal or authz bugs could expose arbitrary files. |
| The SQLite database | Contains usernames, hashes and metadata; must not be web-served. |
| `flex.toml` | May contain bootstrap credentials and filesystem layout. |
| Host/CPU/GPU | An exposed transcode endpoint is a compute/DoS amplifier. |

The relevant adversaries are:

1. **Opportunistic internet scanners** probing for open ports, default
   credentials and known CVEs. Mitigated by TLS, auth on every route, rate
   limiting and not binding to a public interface.
2. **Credential attackers** trying password guessing. Mitigated by Argon2id
   (slow hashing), login rate limiting and 90-day token expiry.
3. **Stolen-token attackers** who obtain one device token. Mitigated by
   individually revocable tokens, per-device metadata (`last_seen_at`) and
   expiry.
4. **Path-traversal / IDOR attackers** abusing item ids, subtitle track numbers
   or artwork types. Mitigated by authorising every stream/artwork/subtitle
   request per user and never concatenating user input into filesystem paths.
5. **Network attackers** on the path to the server. Mitigated by TLS
   termination at the proxy.

Out of scope: a compromised host (root on the server can read everything),
malicious media files exploiting `ffmpeg` (keep `ffmpeg` patched and run the
service as an unprivileged user), and physical access.

## Authentication model

Flex uses **local accounts** with two credential types: passwords (long-lived)
and opaque device tokens (per-device, short-lived).

### Passwords — Argon2id

- Passwords are never stored or logged in plaintext. They are hashed with
  **Argon2id** (the memory-hard winner of the Password Hashing Competition and
  the current recommendation from OWASP/PHC).
- Each hash carries its own random salt and parameters; verification is
  constant-time and transparently upgraded if parameters change.
- Argon2id's memory hardness makes GPU/ASIC offline cracking of a stolen
  database far more expensive than SHA-* or bcrypt.
- Passwords are supplied at account creation (`flex-server user add …` or the
  admin `POST /api/v1/auth/users` endpoint) and changed with
  `flex-server user passwd`.

### Device tokens — opaque, hashed at rest, expiring

- `POST /api/v1/auth/login` returns an **opaque random token**. Clients send it
  as `Authorization: Bearer <token>`.
- Tokens are **stored hashed (SHA-256) at rest**, exactly like passwords: a
  database leak does not directly yield usable bearer tokens.
- Tokens are **per device**. Each login records a `Device` row with a
  human-readable name, `created_at`, `last_seen_at` and `expires_at`, so a user
  can see and revoke individual sessions.
- Tokens **expire after 90 days by default**. Expiry bounds the value of a
  silently stolen token.
- Tokens are **individually revocable**: `POST /api/v1/auth/logout` revokes the
  token in the current request; `DELETE /api/v1/auth/devices/{id}` revokes any
  other device token belonging to the user.
- Tokens are matched by **SHA-256 digest**: the submitted bearer value is hashed
  and looked up against the stored `token_hash` (a unique, indexed column); the
  raw token is never persisted.
- The HLS **session token** (`?t=…`) is a *different*, short-lived secret scoped
  to a single playback session. It authorises only that session's playlist and
  segments, so HLS players that cannot set headers still can't reach anything
  else. See [`TRANSCODING.md`](TRANSCODING.md).

### Login rate limiting

- Failed `POST /api/v1/auth/login` attempts are **rate limited**. The limiter is
  keyed on **(username, peer address)** and returns `429 Too Many Requests`
  after **5 failures within a 15-minute sliding window**; a successful login
  clears the key. Per-username limiting is the primary online defence against
  password spraying on an internet-exposed instance.
- The peer address is the **socket peer** of the connection. When Flex runs
  behind a same-host reverse proxy, that is the proxy (`127.0.0.1`) for every
  client, so the effective key is essentially per-username. This still bounds
  guessing against any given account, but the IP dimension is not a global
  per-attacker cap.
- Forwarding `X-Forwarded-For`/`X-Real-IP` is recommended for accurate
  **logs**, and you should add **proxy-level rate limiting** (Caddy/nginx or a
  WAF) as an additional layer, since Flex does not yet trust forwarded headers
  for limiting.
- Rate limiting is a complement to, not a substitute for, Argon2id: the hash
  protects the store, the limiter protects the login endpoint. Never disable it.

### Authorisation

Every route except `GET /api/v1/health` and `POST /api/v1/auth/login` requires a
valid bearer token. Admin-only routes (user management) check `role == admin`.
**Streams, artwork and subtitle endpoints are authorised per user** — a valid
token from a non-admin account is necessary but the request is still checked
against the item. Do not build custom URL schemes that bypass this check.

## Why exposing Flex directly is risky

Binding Flex straight to a public interface (`host = "0.0.0.0"` plus a firewall
hole or port-forward) means:

- **No transport encryption.** Bearer tokens and passwords cross the network in
  cleartext and are trivially sniffed. There is also no HSTS, so downgrade is
  easy.
- **Direct attack surface.** The application server itself is the only thing
  standing between the attacker and your process; patches, parser bugs and
  DoS amplification (e.g. transcode requests) are all reachable.
- **No TLS, no HTTP/2, no OCSP/HSTS, no automatic cert rotation.**
- **Certificate and hostname handling** belongs in a mature proxy, not in a
  media server.

For these reasons the supported internet-facing topology is:

```
Internet ──TLS(443)──▶ Caddy/nginx (public interface)
                              │  plain HTTP, loopback only
                              ▼
                       flex-server  (host = "127.0.0.1", port 8096)
```

The proxy terminates TLS, obtains/rotates Let's Encrypt certificates, and
forwards to Flex on the loopback interface. See
[`REVERSE_PROXY.md`](REVERSE_PROXY.md) for copy-pasteable configs.

### Bind to loopback

When behind a proxy on the same host, set:

```toml
host = "127.0.0.1"
port = 8096
```

so the only reachable path is through the proxy. Do **not** forward port 8096
from your router/firewall. If the proxy runs on another host, bind to a private
interface and firewall 8096 to the proxy's address only — never the public
internet.

### Keep files out of web roots

Flex serves the web client from its configured `web_dir`. Keep that directory
limited to the client files to avoid accidental exposure:

- **Never put `flex.toml` or the SQLite database (and its `-wal`/`-shm`
  siblings) in a directory served by a web server.** They live under a data
  directory (e.g. `/var/lib/flex`), not under `/var/www`.
- The reverse proxy should forward requests to Flex, which serves both the API
  and the web client. Avoid a separate document root containing config or data.
- Media directories are mounted **read-only** by the service (and, in Docker,
  as `:ro` volumes).
- The systemd unit uses `ProtectSystem=strict` + `ReadWritePaths=/var/lib/flex`
  so the process can only write its own state directory.

## TLS termination at the proxy

- Terminate TLS **only** at the proxy. Flex speaks plain HTTP on loopback.
- Use **Let's Encrypt** (ACME) with automatic renewal. In Caddy this is the
  default; in nginx use certbot.
- Redirect HTTP → HTTPS and enable HSTS once your cert is in place.
- Do not add gzip/brotli compression to media or HLS responses; it wastes CPU
  and can break players. See [`REVERSE_PROXY.md`](REVERSE_PROXY.md).
- Preserve `Authorization` and `Range`/`If-Range` headers when proxying, and
  disable response buffering so streaming starts immediately.

## Hardening checklist

Server / application:

- [ ] `host = "127.0.0.1"` when behind a same-host proxy.
- [ ] Create a dedicated admin with a strong, unique password:
      `flex-server user add admin --admin --password '…'`.
- [ ] Do **not** leave `FLEX_ADMIN_USER`/`FLEX_ADMIN_PASSWORD` set after first
      run — the bootstrap admin is only needed once.
- [ ] Run as an unprivileged, dedicated user (`flex`), not root.
- [ ] Enable the provided systemd hardening (`NoNewPrivileges`,
      `ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp`,
      `ReadWritePaths=/var/lib/flex`).
- [ ] Keep `flex.toml` and the database outside any web root; `chmod 600` the
      database and config where practical.
- [ ] Mount media read-only.
- [ ] Set `RUST_LOG=info` (or `warn`) in production; avoid debug logs that may
      include tokens.
- [ ] Set a sane transcode concurrency limit so a single client cannot pin the
      CPU/GPU or spawn unbounded `ffmpeg` processes.
- [ ] Keep `ffmpeg`/`ffprobe` and the Flex binary patched.

Network / proxy:

- [ ] TLS via Let's Encrypt with automatic renewal.
- [ ] HTTP → HTTPS redirect; HSTS enabled.
- [ ] Firewall: only 80/443 public; 8096 reachable only from the proxy.
- [ ] Do **not** port-forward 8096 from the router.
- [ ] Forward `X-Forwarded-For`/`X-Real-IP` for accurate logs, and add
      proxy-level rate limiting (Flex's limiter keys on the socket peer).
- [ ] `proxy_buffering off` / `flush_interval -1`; no gzip on media.
- [ ] Allow large `Range` requests and long timeouts for streaming.

Accounts / tokens:

- [ ] Per-user accounts; no shared logins.
- [ ] Review `GET /api/v1/auth/devices` periodically; revoke stale devices with
      `DELETE /api/v1/auth/devices/{id}`.
- [ ] Rely on the 90-day default token expiry; log out devices you no longer
      use.
- [ ] Admin role only where needed.

Monitoring:

- [ ] Watch logs for repeated `401`s / rate-limit hits (password guessing).
- [ ] Alert on unexpected transcoder concurrency or high CPU/GPU.

## Reporting a vulnerability

Please report security issues privately rather than opening a public issue, so
a fix can ship before disclosure. See the repository's issue tracker for the
current contact method.
