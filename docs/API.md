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
Streams the underlying file. **Must** support HTTP `Range` requests:

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
