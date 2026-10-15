# Playback, Remux and Transcoding

Flex chooses the cheapest playback strategy that the client can actually
handle. The server never transcodes when it can avoid it: re-encoding costs CPU
or GPU, adds latency, and loses quality. This document explains the decision
model, the HLS session flow, how hardware acceleration is selected, and the
`ffmpeg` command lines used for each mode.

For the wire contract, see the Playback section of [`API.md`](API.md); the
shared types live in `crates/flex-protocol/src/lib.rs`
(`PlaybackMode`, `ClientCapabilities`, `PlaybackSession`).

## The decision model: direct → remux → transcode

When a client calls `POST /api/v1/playback/start` it sends the item id plus its
`ClientCapabilities` (supported containers, video/audio codecs, max
width/height/bitrate, HLS support, HDR support). The server compares those
against the item's probed streams and picks one of three modes, in this order:

| Mode | When it's chosen | Server work | Client gets |
|------|------------------|-------------|-------------|
| **Direct play** | Client supports the container **and** every chosen stream as-is. | None — serve the original file. | A Range-capable file URL (`/api/v1/items/{id}/stream`). |
| **Remux** | Container is unsupported, but the codecs are fine; or only the audio codec is unsupported. | Copy video bit-for-bit; change container and/or re-encode audio. | HLS playlist (MPEG-TS for H.264, fMP4 for HEVC/AV1). |
| **Transcode** | Video codec/profile/resolution/bitrate is unsupported (or HDR can't be rendered). | Decode and re-encode video (and possibly audio), hardware-accelerated when available. | HLS playlist (MPEG-TS for H.264, fMP4 for HEVC/AV1). |

### Why direct and remux are preferred

- **No quality loss.** Direct play and remux copy the encoded video stream
  byte-for-byte. Transcoding is lossy and irreversible.
- **No compute.** Direct play is I/O-bound. Remux only pays the cost of
  repacking (and, at most, one audio transcode). Transcoding is the expensive
  path and the main source of concurrency limits.
- **Instant start.** Direct/remux can begin streaming immediately; transcode
  needs an encode pipeline and segment production before playback.
- **Bandwidth.** Remux does not change the video bitrate, so it can't "fix" a
  slow link — that is what transcoding is for.

The server also prefers the **first** viable mode: if a client can direct-play,
it never remuxes; if it can remux, it never transcodes. Transcoding is a
fallback, not a default.

### Container vs codec

Flex treats the two independently:

- **Container only** (e.g. client plays H.264 but not Matroska) → remux video
  with `-c:v copy` into HLS. The segment container follows the video codec:
  MPEG-TS for H.264, fragmented MP4 for HEVC/AV1.
- **Audio codec only** (e.g. E-AC-3 not supported) → remux with `-c:v copy` and
  re-encode just audio to AAC.
- **Video codec** (e.g. HEVC 10-bit that the client can't decode) → transcode.

Resolution and bitrate also feed the decision: a client advertising
`max_height: 1080` or a low `max_bitrate` forces a transcode even if the codec
is supported.

## HLS session flow

For remux and transcode, the server starts an **HLS session** and returns a
`PlaybackSession` pointing at a master playlist.
The playlist receives an end marker when encoding finishes so players can
report a finite duration, allow seeking, and detect completion.

```
Client                     flex-server                         ffmpeg
  │  POST /playback/start      │                                  │
  │  {item_id, capabilities}   │                                  │
  ├───────────────────────────▶│  compare capabilities vs item    │
  │                            │  choose mode (direct/remux/xcode) │
  │                            │                                  │
  │                            │  create session_id + short-lived  │
  │                            │  session_token (scoped)           │
  │                            ├─ spawn ──────────────────────────▶│
  │  PlaybackSession {url,…}   │                                  │ write .m3u8 + segments
  │◀───────────────────────────┤                                  │
  │                            │                                  │
  │  GET master.m3u8?t=…       │                                  │
  ├───────────────────────────▶│  validate t= (session scoped)    │
  │  playlist                  │  serve from session cache dir    │
  │◀───────────────────────────┤                                  │
  │  GET media-000.ts?t=…      │                                  │
  ├───────────────────────────▶│  serve segment                   │
  │◀───────────────────────────┤                                  │
  │  ... play ...              │                                  │
  │  POST /playback/stop       │                                  │
  ├───────────────────────────▶│  kill ffmpeg, delete session dir │
```

### Scoped `?t=` session tokens

HLS players (including Android's ExoPlayer and most browser players) request
playlists and segments with **no custom headers**, so the bearer token cannot be
attached to every segment request. Flex solves this with a **scoped session
token**:

- The session URL returned by `/playback/start` already contains
  `?t=<session_token>`, e.g.
  `/api/v1/playback/b3f1…/master.m3u8?t=…`.
- Served playlists also include the token on media segment and initialization
  URLs. A relative segment URL does not inherit the playlist's query string.
  This lets native HLS players authenticate every request without custom headers.
- The token authorises **only that session's** playlist and segment files. It
  cannot be used to reach other items or other endpoints.
- It is short-lived and tied to `PlaybackSession.expires_at`; unknown or expired
  sessions return `404`.
- Segment objects (`GET /api/v1/playback/{session_id}/{file}`) are served from
  the session's cache directory only — the file component is never used to
  build an arbitrary path.

This is why the bearer token from `/auth/login` and the `?t=` session token are
distinct: the bearer token authenticates the *user*; the session token
authorises *one stream* for a player that can't send headers.

## Hardware acceleration selection

At startup (and exposed via `GET /api/v1/system/info`) Flex probes `ffmpeg` for
what the host actually supports. It does **not** assume a GPU is present.

```sh
# Which acceleration APIs are available (e.g. cuda, vaapi, qsv, …)?
ffmpeg -hide_banner -hwaccels

# Which encoders are compiled in and available (e.g. h264_nvenc, hevc_nvenc,
# h264_vaapi, libx264 …)?
ffmpeg -hide_banner -encoders
```

The results are surfaced as `SystemInfo.hwaccels` and `SystemInfo.hw_encoders`.
Flex picks one encoder per family — H.264 and HEVC independently — by walking a
fixed preference list and taking the first that is present:

1. **NVENC** — `h264_nvenc` / `hevc_nvenc`.
2. **VAAPI** — `h264_vaapi` / `hevc_vaapi`.
3. **QSV** — `h264_qsv` / `hevc_qsv`.
4. **AMF** — `h264_amf` / `hevc_amf`.
5. **Software fallback** — `libx264` for H.264, `libx265` for HEVC (always
   available).

Transcode output is always H.264 or HEVC; AV1 only appears as a source codec
that a remux may copy through.

A hardware encoder is only used when one of the above is actually reported by
`ffmpeg -encoders`. When hardware is chosen, decoding is accelerated too, using
the first available of `cuda`, `vaapi`, `qsv`, `d3d11va`, `videotoolbox` (passed
as `-hwaccel`).

If `ffmpeg` is missing or transcoding is disabled, `/playback/start` can still
serve direct/remux; transcode requests are refused. `SystemInfo.transcode_enabled`
reflects this.

### This deployment: RTX 2070 SUPER (Turing)

The target server has an **NVIDIA RTX 2070 SUPER** (Turing, TU104). Use
**NVENC** for H.264 and HEVC when the GPU is passed through to the server:

- Encoders: **`h264_nvenc`** and **`hevc_nvenc`**.
- **AV1 encode is NOT available on Turing.** AV1 hardware encoding starts with
  the RTX 40-series (Ada). Do not request `av1_nvenc` on this GPU; a software
  AV1 encoder would be far too slow and is not used.
- A single 2070 SUPER handles **multiple concurrent 1080p NVENC sessions**
  comfortably — NVENC is an independent ASIC and does not consume CUDA cores.
  The practical limit on this hardware is usually the **consumer-driver
  concurrent-session cap** (historically 2 simultaneous NVENC sessions on
  GeForce consumer drivers), not the GPU itself. Flex's transcode concurrency
  limit should therefore be set conservatively (see below).
- Always keep the **`libx264`/`libx265` software fallback** available for when no
  GPU is present (e.g. VMs, CPU-only hosts, or a GPU that is busy/unavailable).

Enable hardware transcoding only when a GPU is passed through to the server:

- systemd: `DeviceAllow`/`ProtectDevice` adjustments or running without a
  restrictive device policy, plus the NVIDIA container/runtime.
- Docker: pass the GPU with `--gpus all` (or a Compose `deploy.resources`
  GPU reservation) and the NVIDIA Container Toolkit. **Do not enable this by
  default** — the image should start and run CPU-only, using `libx264`/`libx265`,
  with no GPU device requested. See [`DEPLOYMENT.md`](DEPLOYMENT.md).

## Example `ffmpeg` command lines

These are illustrative of the shape Flex uses. Segment naming, cache paths and
exact flags are implementation details; the `-c:v` decision and the HLS segment
type are the core. Every HLS command adds:

```
-f hls -hls_segment_type <mpegts|fmp4> -hls_time 6 -hls_playlist_type event
-hls_flags independent_segments+append_list -hls_segment_filename <name> master.m3u8
```

plus `-hls_fmp4_init_filename init.mp4` when segmenting as fMP4.

### (a) Remux-only HLS (copy video, re-encode audio to AAC if needed)

Used when the container or audio codec is unsupported but the video is fine.
Segment type follows the **source** video codec: `.ts` for H.264, `.m4s` +
init segment for HEVC/AV1.

```sh
# H.264 source → MPEG-TS segments
ffmpeg -hide_banner -nostdin -i input.mkv \
  -map 0:v:0 -c:v copy \
  -map 0:a:0 -c:a aac -b:a 192k -ac 2 \
  -f hls -hls_segment_type mpegts -hls_time 6 \
  -hls_playlist_type event \
  -hls_flags independent_segments+append_list \
  -hls_segment_filename 'seg_%05d.ts' \
  master.m3u8

# HEVC/AV1 source → fragmented MP4 segments
ffmpeg -hide_banner -nostdin -i input.mkv \
  -map 0:v:0 -c:v copy \
  -map 0:a:0 -c:a aac -b:a 192k -ac 2 \
  -f hls -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_time 6 \
  -hls_playlist_type event \
  -hls_flags independent_segments+append_list \
  -hls_segment_filename 'seg_%05d.m4s' \
  master.m3u8
```

If audio is already compatible (e.g. AAC) it is copied too (`-c:a copy`), making
this a pure repack. When resuming, Flex prepends `-ss <seconds>` **before** `-i`
for a fast input seek.

### (b) NVENC H.264 transcode with scaling

Used when the client advertises a lower max resolution/bitrate or an
unsupported video codec, and an NVIDIA GPU is available.

```sh
ffmpeg -hide_banner -nostdin \
  -hwaccel cuda -i input.mkv \
  -map 0:v:0 -map 0:a:0 \
  -c:v h264_nvenc -preset p4 -tune hq -rc vbr \
  -cq 23 -b:v 8M -maxrate 12M -bufsize 16M \
  -vf "scale=1920:-2" \
  -c:a aac -b:a 192k -ac 2 \
  -f hls -hls_segment_type mpegts -hls_time 6 \
  -hls_playlist_type event \
  -hls_flags independent_segments+append_list \
  -hls_segment_filename 'seg_%05d.ts' \
  master.m3u8
```

Notes:

- `-hwaccel cuda` accelerates **decoding** where supported; `h264_nvenc` is the
  **encoder**. Flex only passes `-hwaccel` when it selected a hardware encoder.
- `scale=1920:-2` caps width at 1080p and lets ffmpeg keep the aspect ratio
  (the `-2` keeps the height even, required by most encoders). When both a max
  width and height are known, Flex emits
  `scale=w='min(w,iw)':h='min(h,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2`
  instead.
- H.264 output is segmented as MPEG-TS. For HEVC sources where the client
  supports H.265, `hevc_nvenc` is used instead and the output is segmented as
  fMP4 (with `-hls_fmp4_init_filename init.mp4` and `.m4s` segments).
- Software encoders use `-preset veryfast -crf 23`; hardware encoders use
  `-preset p4 -tune hq -rc vbr -cq 23`.

### (c) Software `libx264` fallback

Identical decision logic, CPU encoder. Always available; the safety net when no
GPU is passed through.

```sh
ffmpeg -hide_banner -nostdin -i input.mkv \
  -map 0:v:0 -map 0:a:0 \
  -c:v libx264 -preset veryfast -crf 23 \
  -vf "scale=1280:-2" \
  -c:a aac -b:a 192k -ac 2 \
  -f hls -hls_segment_type mpegts -hls_time 6 \
  -hls_playlist_type event \
  -hls_flags independent_segments+append_list \
  -hls_segment_filename 'seg_%05d.ts' \
  master.m3u8
```

For an HEVC output the software fallback is `libx265` and the segments are fMP4.
The software encoder is significantly slower than NVENC — expect it to be the
bottleneck for concurrent sessions on a CPU-only host.

## Session cache, cleanup and concurrency limits

- **Session directories.** Each HLS session writes its playlist, init segment
  and media segments into a per-session directory under the configured cache
  directory (`cache_dir` in `flex.toml`, defaulting to `./data/cache`).
- **Cleanup on stop.** `POST /api/v1/playback/stop` kills the `ffmpeg` process
  and removes the session directory.
- **Cleanup on expiry / disconnect.** Sessions carry an `expires_at` four hours
  out. A background GC task (every 30 s) reclaims sessions that have seen no file
  access for two minutes or have outlived their TTL, killing any lingering
  `ffmpeg` child and deleting its files, so abandoned sessions do not fill the
  disk.
- **Restart hygiene.** Stale session directories from a previous run are
  swept on startup.
- **Concurrency limits.** Every **HLS** session — remux *and* transcode — counts
  against the configurable `max_transcode_sessions` limit, because both run an
  `ffmpeg` process. Direct-play sessions are simple byte-range file serves and
  are not counted. Size the limit to your hardware:
  - RTX 2070 SUPER: a couple of concurrent NVENC transcodes is a safe default
    given the consumer-driver session cap; `libx264` fallback is even lower.
  - Watch `GET /api/v1/system/info` and server logs for rejected transcode
    requests when the limit is hit.

Keep the cache directory on a fast disk with enough free space; each active
session holds live segments plus completed ones for the whole stream.
