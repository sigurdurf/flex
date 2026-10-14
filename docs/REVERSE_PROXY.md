# Running Flex behind a reverse proxy

The supported way to expose Flex to the internet is to terminate TLS at a
reverse proxy (Caddy or nginx) and forward to Flex on the loopback interface.
This gives you automatic HTTPS with Let's Encrypt, a mature TLS stack, and keeps
the application server off the public interface.

```
Internet ──TLS(443)──▶ Caddy / nginx ──HTTP(127.0.0.1:8096)──▶ flex-server
```

Set the server to loopback so the only path in is the proxy:

```toml
# flex.toml
host = "127.0.0.1"
port = 8096
```

> **Do not port-forward 8096.** Only 80 and 443 should be public. If the proxy
> runs on a different host, bind Flex to a private address and firewall 8096 to
> the proxy only.

For the security rationale, see [`SECURITY.md`](SECURITY.md).

## Caddy (recommended — automatic HTTPS)

Caddy obtains and renews Let's Encrypt certificates automatically and enables
HTTP→HTTPS redirects by default. Install Caddy, point an `A`/`AAAA` record for
your hostname at the server, then use a `Caddyfile` like this:

```caddyfile
media.example.com {
	# Stream immediately: disable response buffering so playback and HLS
	# segment requests are not held back. `-1` means "flush on every write".
	reverse_proxy 127.0.0.1:8096 {
		flush_interval -1
	}

	# Long-lived streaming connections: raise timeouts above Caddy's defaults
	# only if your clients stall on long seeks. These are generous, not required.
	# (Caddy streams fine with defaults for most libraries.)

	# Do NOT enable `encode gzip` here. Compressing video/HLS wastes CPU and
	# can break players that expect exact byte ranges.
}
```

Notes:

- **Don't add gzip/brotli to media.** If you want compression for JSON, scope
  it deliberately, e.g. `encode gzip` only on specific JSON paths — but the
  simplest correct choice is no compression at all.
- **Range requests** are passed through untouched; Caddy does not buffer by
  default, and `flush_interval -1` makes HLS/streaming latency minimal.
- **Large requests**: Caddy has no default body size limit for proxied
  requests, so uploads/range requests are fine.
- Reload after editing: `sudo systemctl reload caddy`.
- Caddy forwards `X-Forwarded-For` automatically (useful for logs). Flex's own
  login rate limiter keys on the socket peer address, so if you want a
  per-client cap, add a Caddy `rate_limit` directive or another proxy-level
  limiter.

## nginx (alternative)

Use this when nginx is already part of your stack. Obtain certificates with
certbot (`certbot --nginx`) or configure ACME manually, then:

```nginx
# Redirect all plain HTTP to HTTPS.
server {
    listen 80;
    listen [::]:80;
    server_name media.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name media.example.com;

    ssl_certificate     /etc/letsencrypt/live/media.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/media.example.com/privkey.pem;

    # Streaming: correct protocol, no body-size cap, long timeouts.
    client_max_body_size 0;

    location / {
        proxy_pass http://127.0.0.1:8096;

        # Keep-alive and correct Host/forwarding info.
        proxy_http_version 1.1;
        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Connection        "";

        # Preserve auth and range semantics end to end.
        proxy_set_header Authorization $http_authorization;
        proxy_set_header Range         $http_range;
        proxy_set_header If-Range      $http_if_range;

        # Do not buffer request or response bodies — stream as bytes arrive.
        proxy_buffering off;
        proxy_request_buffering off;

        # Long-lived streams and large seeks.
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_connect_timeout 30s;

        # Never compress media/HLS.
        gzip off;
    }

    # Explicitly refuse to serve config/data if it ever lands in a web root.
    location ~* \.(toml|db|db-wal|db-shm)$ {
        deny all;
    }
}
```

Then `sudo nginx -t && sudo systemctl reload nginx`.

Notes:

- `proxy_buffering off` and `proxy_request_buffering off` are the important
  streaming settings: without them nginx may buffer whole responses, adding
  latency and memory pressure for large files.
- `proxy_http_version 1.1` plus `Connection ""` enables keep-alive to the
  upstream, which matters for HLS (many small segment requests).
- `client_max_body_size 0` removes nginx's 1&nbsp;MB default that would otherwise
  reject large `Range`/`PUT`-style bodies.
- `gzip off` prevents CPU burn and byte-range corruption on video.

## DNS and Let's Encrypt

1. Create an `A` (and `AAAA` if you have IPv6) record for `media.example.com`
   pointing at the server's public IP. Wait for propagation
   (`dig +short media.example.com`).
2. Ensure inbound **80** and **443** are open; ACME HTTP-01 (and TLS-ALPN-01)
   need them. If 80 is blocked, use DNS-01.
3. Caddy: certificates are automatic. nginx: run
   `sudo certbot --nginx -d media.example.com` and verify auto-renewal with
   `sudo certbot renew --dry-run`.
4. Redirect HTTP→HTTPS and enable HSTS after the cert works.

## Verify it works

With Flex running behind the proxy and an admin account created, exercise the
full path — TLS, login, bearer auth and a `Range` request — through the proxy.

```sh
# 1. TLS + health (no auth required).
curl -sS https://media.example.com/api/v1/health
# {"status":"ok","version":"0.1.0","api_version":"v1"}

# 2. Log in and capture the device token.
TOKEN=$(curl -sS -X POST https://media.example.com/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"YOUR_PASSWORD","device_name":"curl"}' \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["token"])')

# 3. Authenticated listing must be 200.
curl -sS -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer ${TOKEN}" \
  https://media.example.com/api/v1/items            # -> 200

# 4. Unauthenticated listing must be 401.
curl -sS -o /dev/null -w '%{http_code}\n' \
  https://media.example.com/api/v1/items            # -> 401

# 5. A Range request through the proxy must return 206 Partial Content.
#    (Pick a real item id from step 3.)
curl -sS -D - -o /dev/null \
  -H "Authorization: Bearer ${TOKEN}" \
  -H 'Range: bytes=0-99' \
  https://media.example.com/api/v1/items/1/stream | sed -n '1,12p'
# Expect: HTTP/2 206, Content-Range: bytes 0-99/…, Accept-Ranges: bytes
```

If step 5 shows `200` instead of `206`, or stalls, the proxy is buffering or
stripping `Range` — revisit the streaming settings above.
