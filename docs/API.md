# Flex HTTP API — v1

Base URL: `http://<host>:<port>` (default port `8096`).
All endpoints are under `/api/v1` unless noted. All request/response bodies are
JSON except the byte-stream and subtitle endpoints. The canonical type
definitions live in the `flex-protocol` crate (`crates/flex-protocol/src/lib.rs`);
this document describes the endpoint contract.

## Conventions

- Timestamps are Unix time in **seconds** (`i64`).
- Durations are milliseconds (`u64`).
- Errors use HTTP status codes and a body of `{ "error": "<code>", "message": "<detail>" }`.
- List endpoints are paginated with `limit`/`offset` and return a `Page<T>`:
  `{ "items": [...], "total": <i64>, "limit": <i64>, "offset": <i64> }`.

## Authentication

Most routes require a bearer token:

```
Authorization: Bearer <device-token>
```

The unauthenticated exceptions are `GET /health`, `POST /auth/login` and the
static web client (any non-`/api` path). In addition, media-fetching routes can
be authorised by a **scoped session token** instead of the bearer token, because
plain HLS players cannot attach custom headers:

- HLS playlist/segment routes (`/playback/{session_id}/…`) accept `?t=<session_token>`.
- The direct-play stream route (`/items/{id}/stream`) accepts **either** a bearer
  token **or** a `?t=<session_token>` bound to that item.

Tokens are **opaque**, random, stored hashed (SHA-256) at rest, and individually
revocable per device. `POST /auth/login` returns the token once; the client must
persist it (Android Keystore / libsecret). Failed logins are rate limited.

### `POST /api/v1/auth/login`
Body: `LoginRequest` `{ "username", "password", "device_name"? }`.
Returns `LoginResponse` `{ "token", "user", "expires_at" }`, or `401`.

### `POST /api/v1/auth/logout`
Revokes the token used for the request. Returns `204`.

### `GET /api/v1/auth/me`
Returns the authenticated `User`.

### `GET /api/v1/auth/devices`
Returns `Device[]` for the current user (`current: true` marks this token).

### `DELETE /api/v1/auth/devices/{id}`
Revokes another of the user's device tokens. Returns `204`.

### Admin user management
`POST /api/v1/auth/users` (body `CreateUserRequest`) and `GET /api/v1/auth/users`
are restricted to `admin` users. Initial admins are created from the CLI
(`flex-server user add <name> --admin`).

## Playback

### `POST /api/v1/playback/start`
Body: `PlaybackStartRequest` — the item id plus the client's
`ClientCapabilities`. The server chooses:

- **direct** — client can decode container + all streams as-is: returns a
  Range-capable file URL (`/items/{id}/stream`).
- **remux** — container unsupported but codecs are fine (or only the audio
  codec is unsupported): video is copied, container/audio re-muxed, served as HLS.
- **transcode** — video codec/profile/resolution/bitrate unsupported: re-encode
  (hardware accelerated when available). Both HLS modes segment with MPEG-TS for
  H.264 output and fragmented MP4 for HEVC/AV1 output.

Returns `PlaybackSession`:
```json
{
  "session_id": "b3f1…",
  "mode": "remux",
  "stream_kind": "hls",
  "url": "/api/v1/playback/b3f1…/master.m3u8?t=<session_token>",
  "mime": "application/vnd.apple.mpegurl",
  "duration_ms": 9792000,
  "audio_stream_index": 1,
  "subtitle_stream_index": null,
  "session_token": "…",
  "expires_at": 1710003600
}
```

### `POST /api/v1/playback/stop`
Body: `StopPlaybackRequest`. Terminates the ffmpeg process if any. Returns `204`.

### `GET /api/v1/playback/{session_id}/master.m3u8` and `/playback/{session_id}/{file}`
Serve the HLS playlist, init segment and media segments. These are authorised by
the scoped `t=<session_token>` query parameter (not the bearer token) so any HLS
player works without custom headers. Served playlists include this token on
each segment and initialization URI because relative URLs do not inherit the
playlist URL's query parameters. Playlist byte ranges and content lengths
describe the resulting signed content. Unknown/expired sessions return `404`.

## System

### `GET /api/v1/system/info`
Returns `SystemInfo`: version, detected `hwaccels`, available hardware encoders,
and whether transcoding is enabled.

## Watch state / progress

All progress is scoped to the authenticated user.

### `GET /api/v1/continue-watching?limit=&offset=`
Returns `Page<ContinueWatchingEntry>`: items with saved progress that are not
yet finished, most-recently-watched first. Each entry is `{ "item": MediaItem,
"state": WatchState }`.

### `GET /api/v1/progress`
Returns `WatchState[]` for the current user.

### `GET /api/v1/progress/{item_id}`
Returns a single `WatchState`, or `404` if none recorded.

### `PUT /api/v1/progress/{item_id}`
Body: `UpdateProgressRequest` `{ "position_ms", "duration_ms"?, "watched"? }`.
Upserts and returns the resulting `WatchState`. Clients send this periodically
during playback and on stop. When `watched` is omitted the server sets it once
`position_ms >= 90%` of `duration_ms`.

### `DELETE /api/v1/progress/{item_id}`
Clears progress for the item (mark unwatched). Returns `204`.

> **Resume:** `POST /api/v1/playback/start` uses `start_position_ms` when given;
> if omitted, the server resumes from the user's saved `WatchState` position
> (unless the item is already marked watched).

## Subtitle search

### `GET /api/v1/subtitles/search`
Query params: `query`, `languages` (repeatable or comma-separated),
`item_id`, `season`, `episode`. Returns `SubtitleSearchResult[]`. When `item_id`
is supplied and `query` is absent the server derives the query from the item's
title/year. Providers are configured server-side (e.g. OpenSubtitles API key).

### `POST /api/v1/subtitles/download`
Body: `SubtitleDownloadRequest` `{ "provider", "id", "language"?, "item_id"? }`.
When `item_id` is set the subtitle is converted to WebVTT and written next to
the media file as a sidecar, then attached to the item. Returns
`SubtitleDownloadResult`. Returns `400` if the provider is unknown, `404` if the
subtitle cannot be found, and `502` on an upstream provider error.

## Endpoints

### `GET /api/v1/health`
Liveness probe. Returns `Health`: `{ "status": "ok", "version": "0.1.0", "api_version": "v1" }`.

### `GET /api/v1/libraries`
Returns an array of `Library`:
```json
[{
  "id": 1,
  "name": "Movies",
  "path": "/srv/media/movies",
  "kind": "movies",
  "created_at": 1710000000,
  "item_count": 42
}]
```

### `POST /api/v1/scan`
Walks every configured library, runs `ffprobe`, and upserts items.
Returns `ScanReport`:
```json
{ "libraries": 2, "scanned": 128, "added": 3, "updated": 1, "removed": 0, "errors": [] }
```

### `GET /api/v1/items`
Browse indexed media. Query parameters (all optional):

| param        | type   | notes                                                              |
|--------------|--------|--------------------------------------------------------------------|
| `library_id` | int    | Filter to one library.                                             |
| `kind`       | string | `movie` \| `episode` \| `other`.                                   |
| `q`          | string | Case-insensitive title substring match.                            |
| `sort`       | string | `title` \| `-title` \| `year` \| `-year` \| `added` \| `-added`.   |
| `limit`      | int    | Default `50`, max `500`.                                            |
| `offset`     | int    | Default `0`.                                                        |

Returns `Page<MediaItem>`.

### `GET /api/v1/items/{id}`
Returns a single `MediaItem` or `404`.

### `GET /api/v1/items/{id}/stream`
Streams the underlying file. Accepts a bearer token or a direct-play
`?t=<session_token>` scoped to this item. **Must** support HTTP `Range` requests:

- `200` with `Accept-Ranges: bytes` when no range is requested.
- `206 Partial Content` with `Content-Range` for `Range: bytes=start-end`.
- `416` for unsatisfiable ranges.
- `Content-Type` derived from the container (`video/mp4`, `video/x-matroska`, …).

### `GET /api/v1/items/{id}/artwork?type=poster|backdrop`
Returns image bytes if artwork exists, otherwise `404`. `type` defaults to `poster`.

### `GET /api/v1/items/{id}/subtitles`
Returns an array of `SubtitleStream`.

### `GET /api/v1/items/{id}/subtitles/{track}`
Returns the extracted subtitle track as `text/vtt` (WebVTT). The server extracts
embedded subtitles with `ffmpeg` on demand; external sidecar files are served directly.

## MediaItem shape
```json
{
  "id": 7,
  "library_id": 1,
  "kind": "movie",
  "title": "Blade Runner 2049",
  "sort_title": "blade runner 2049",
  "year": 2017,
  "overview": null,
  "file_path": "/srv/media/movies/Blade Runner 2049 (2017).mkv",
  "file_size": 34359738368,
  "container": "matroska,webm",
  "duration_ms": 9792000,
  "video": { "codec": "hevc", "width": 3840, "height": 2160, "fps": 23.976, "bitrate": 26000000 },
  "audio": [{ "index": 1, "codec": "eac3", "channels": 6, "language": "eng", "title": "Surround 5.1", "bitrate": 768000 }],
  "subtitles": [{ "index": 2, "codec": "subrip", "language": "eng", "title": null, "forced": false, "default": true }],
  "added_at": 1710000000,
  "updated_at": 1710000000,
  "poster_url": "/api/v1/items/7/artwork?type=poster",
  "backdrop_url": null
}
```
