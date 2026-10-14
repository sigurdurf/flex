# Flex Web UI

A self-contained, no-build-step single-page app for browsing and playing media
from a Flex server. Plain HTML, CSS and vanilla JavaScript — no bundler, no npm,
no compilation. The only third-party dependency is a locally vendored copy of
[hls.js](`vendor/hls.js`).

## Files

- `index.html` — app shell (login, browse grid, detail view, player overlay).
- `styles.css` — dark, responsive media-browser styling.
- `app.js` — all application logic (auth, browsing, detail, playback, progress,
  continue-watching, subtitle search).
- `vendor/hls.js` — vendored hls.js build used for HLS playback in browsers that
  lack native HLS support (Safari falls back to native `video.src`).

## How it is served

The Flex server serves this directory at `/` as static files:

- `/` → `web/index.html`
- `/styles.css`, `/app.js`, `/vendor/hls.js` → static assets

Open `http://<host>:<port>/` in a browser. The server base URL field defaults to
the current origin, so when served by `flex-server` at `/` no configuration is
needed. All API requests go to `<server>/api/v1/...` with
`Authorization: Bearer <token>`.

## Storage

- The selected server URL is persisted in `localStorage` under `flex.server`.
- The bearer token is stored per server under `flex.token:<server-url>`.

## API endpoints used

Authentication:

- `POST /api/v1/auth/login`
- `GET /api/v1/auth/me`
- `POST /api/v1/auth/logout`

Library / browse:

- `GET /api/v1/libraries`
- `GET /api/v1/items` (with `library_id`, `q`, `limit`, `offset`)
- `GET /api/v1/items/{id}`
- `GET /api/v1/items/{id}/artwork?type=poster` (fetched as a blob with the bearer
  token, then shown via an object URL)

Playback:

- `POST /api/v1/playback/start` (with browser `ClientCapabilities`)
- `POST /api/v1/playback/stop`

Progress / continue watching:

- `GET /api/v1/continue-watching`
- `GET /api/v1/progress/{item_id}`
- `PUT /api/v1/progress/{item_id}` (every ~10s, on pause/ended, and on unload)

Subtitles:

- `GET /api/v1/subtitles/search?item_id={id}`
- `POST /api/v1/subtitles/download`

## Notes

- A `401` from any request clears the stored token and returns to the login
  screen.
- Artwork is loaded with `fetch` + bearer token and rendered through a blob
  object URL, since `<img>` cannot send an `Authorization` header.
- The player saves progress on a 10 second timer, and on pause, end, tab hide
  and page unload (using `keepalive` requests).
