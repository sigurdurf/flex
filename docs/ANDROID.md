# Flex Android client

> **Status: builds and lints clean.** The sources under `android/` are compiled
> by CI (Gradle `assembleDebug lintDebug` on JDK 17 + SDK 35) and have also been
> built locally as a debug APK and an R8-minified release APK, with 0 lint
> errors. On-device runtime behaviour (login against a live server, actual
> playback) has **not** been exercised — see
> [`android/README.md`](../android/README.md#verification). The Linux desktop
> client (`cargo run -p flex-client`) and the web client remain the
> most-verified clients.

The Android client is a native app that talks to the same Flex HTTP API as the
desktop client and any `curl` script. It browses libraries/items, authenticates
per user, stores a device token, and plays media either by **direct play** or
over **HLS**.

## Prerequisites

| Tool | Version |
|------|---------|
| JDK | **17** |
| Android SDK Platform | **35** (compileSdk 35) |
| Android Studio | Latest stable (Ladybug or newer) |
| Gradle | Via the bundled wrapper (`gradlew`) |

Set `ANDROID_HOME` (or `ANDROID_SDK_ROOT`) to your SDK location and install
platform 35 and the matching build-tools through the SDK Manager.

```sh
export ANDROID_HOME="$HOME/Android/Sdk"
export PATH="$ANDROID_HOME/platform-tools:$PATH"
```

## Build and run

Open the `android/` directory as a project in Android Studio and run the `app`
configuration on an emulator or device, **or** build from the command line using
the Gradle wrapper:

```sh
# From the android/ directory:
./gradlew assembleDebug          # build a debug APK
./gradlew installDebug           # build + install on a connected device/emulator
```

The debug APK is written under `app/build/outputs/apk/debug/`. Use
`adb devices` to confirm a target is connected. Use `./gradlew assembleRelease`
for an R8-minified (unsigned) release APK and `./gradlew lintDebug` for Android
Lint. The Gradle wrapper is committed, and the toolchain versions
(AGP 8.6.1, Gradle 8.7, Kotlin 2.0.20, Media3 1.4.1) are pinned in
[`android/gradle/libs.versions.toml`](../android/gradle/libs.versions.toml).

## Configuring the server URL

The app needs the Flex base URL (scheme + host + port), for example
`https://media.example.com` or, on a LAN, `http://192.168.1.10:8096`.

- In the running app, set it on the login screen / settings field before
  logging in.
- During early development the default is typically read from a build constant
  or a `BuildConfig` field so a debug build can point at a local server without
  retyping it.

Point production installs at your **TLS reverse proxy** hostname, never a raw
public IP:port. See [`REVERSE_PROXY.md`](REVERSE_PROXY.md).

## Login and token storage

1. The app sends `POST /api/v1/auth/login` with the username, password and a
   device name identifying the phone/tablet.
2. The server returns a `LoginResponse` containing an **opaque bearer token**
   plus the `User` and an optional `expires_at` (90 days by default).
3. The token is persisted on-device and sent as
   `Authorization: Bearer <token>` on every subsequent request.

Storage rules:

- **Never store the token in plain SharedPreferences.** Use the Android
  **Keystore**-backed storage (`EncryptedSharedPreferences` from
  `androidx.security` or a Keystore-wrapped key) so the token is encrypted at
  rest and bound to the device.
- Never log the token.
- Handle `401` centrally: clear the stored token and return to the login
  screen. A `401` means the token expired or was revoked from another device
  (`GET /api/v1/auth/devices` / `DELETE …/devices/{id}`).
- Provide a "log out" action that calls `POST /api/v1/auth/logout` and clears
  local storage.

## Playback: direct vs HLS

Playback mode is chosen **by the server**, not the app. The app advertises what
it can decode in `ClientCapabilities` when calling
`POST /api/v1/playback/start`, and the returned `PlaybackSession` says what to
do:

- `mode = "direct"`, `stream_kind = "file"` → the app plays the returned
  Range-capable file URL directly (e.g. via `Media3`/ExoPlayer), relying on HTTP
  `Range` for seeking. No server-side encoding.
- `mode = "remux"` or `mode = "transcode"`, `stream_kind = "hls"` → the app
  plays the returned HLS master playlist URL with any HLS-capable player
  (`Media3`/ExoPlayer's HLS support).

The HLS URL already carries the scoped `?t=<session_token>` query parameter, so
the player fetches playlists and segments **without** adding auth headers. Do
not strip the query string.

Declare realistic `ClientCapabilities` (containers, `h264`/`hevc`/`av1` decode
support, AAC audio, max width/height, `hls: true`). Advertising more than the
device can really decode causes the server to direct-play something the device
then fails to render; advertising less forces unnecessary transcoding.

The app should call `POST /api/v1/playback/stop` when playback ends so the
server can kill `ffmpeg` and free the session. See
[`TRANSCODING.md`](TRANSCODING.md) for how the server decides and what it runs.

## Cleartext traffic and network security

Android blocks cleartext (plain HTTP) by default since API 28. For **LAN
testing only**, the app ships a `network_security_config` with a cleartext
exception for local addresses, e.g.:

```xml
<network-security-config>
    <domain-config cleartextTrafficPermitted="true">
        <domain includeSubdomains="false">192.168.1.10</domain>
        <domain includeSubdomains="false">10.0.2.2</domain> <!-- emulator host -->
    </domain-config>
</network-security-config>
```

This exception exists **only** so a developer can reach a server on the local
network over `http://`. It must be narrow (a concrete IP, never a wildcard) and
must not be used for production.

**Production must go through the TLS reverse proxy.** Configure the app with an
`https://` URL and leave cleartext disabled globally. If the deployment also has
a LAN-facing HTTP listener, keep the cleartext exception scoped to private
address ranges and never to `0.0.0.0/0`. See [`SECURITY.md`](SECURITY.md) and
[`REVERSE_PROXY.md`](REVERSE_PROXY.md).
