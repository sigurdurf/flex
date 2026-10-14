# Flex — Android client

A native Android client for **Flex** (an open-source Plex alternative). It talks
to a `flex-server` over the HTTP API described in
[`docs/API.md`](../docs/API.md), whose wire types live in
[`crates/flex-protocol`](../crates/flex-protocol/src/lib.rs).

> **UNVERIFIED — not compiled in this environment.**
> This project was scaffolded in an environment with **no JDK, Gradle, Android
> SDK, or Kotlin toolchain**, so it has **not** been built, linted, or run. The
> source is written to be conventional and internally consistent, but treat the
> first `./gradlew assembleDebug` as the real test. See
> [Unverified / risky](#unverified--risky) below.

## Features

1. **Login** — server URL (default `http://127.0.0.1:8096`), username, password,
   optional device name. `POST /api/v1/auth/login`; the bearer token is stored in
   `EncryptedSharedPreferences`.
2. **Browse** — library sidebar + adaptive poster grid from `GET /api/v1/items`
   (paginated) with posters from `GET /api/v1/items/{id}/artwork?type=poster`.
   The search box maps to the `q` query parameter.
3. **Detail** — title, year, container, duration, and video/audio/subtitle track
   listings, plus a Play button.
4. **Player** — Media3 `ExoPlayer` in a `PlayerView`. On Play it calls
   `POST /api/v1/playback/start` advertising typical Android decoder
   capabilities. The returned `url` (a direct file URL or an HLS master playlist
   carrying `?t=<session_token>`) is handed straight to ExoPlayer.
5. **Logout** — `POST /api/v1/auth/logout`, clears the stored token, returns to
   login.

## Prerequisites

- **Android Studio** Ladybug (2024.2) or newer, **or** a standalone JDK 17 +
  Android command-line tools.
- **Android SDK Platform 35** and **Build-Tools** for it (install via SDK
  Manager).
- **Gradle 8.7+**. The wrapper is expected (see below).
- A reachable `flex-server` instance.

## Toolchain

| Component            | Version      |
|----------------------|--------------|
| Android Gradle Plugin| 8.6.1        |
| Gradle               | 8.7          |
| Kotlin               | 2.0.20       |
| Compose BOM          | 2024.09.03   |
| Compose compiler     | `org.jetbrains.kotlin.plugin.compose` 2.0.20 |
| Navigation Compose   | 2.8.0        |
| Media3               | 1.4.1        |
| Retrofit             | 2.11.0       |
| OkHttp               | 4.12.0       |
| kotlinx.serialization| 1.7.3        |
| security-crypto      | 1.1.0-alpha06|
| Coil                 | 2.7.0        |
| compileSdk / targetSdk | 35         |
| minSdk               | 26           |

Versions are centralized in [`gradle/libs.versions.toml`](gradle/libs.versions.toml).

## Opening / building

There is **no `gradle-wrapper.jar`** in this repository (binaries are not
generated here). Generate it once with a local Gradle, or just let Android
Studio do it:

```bash
# From the android/ directory, with Gradle 8.7+ on your PATH:
gradle wrapper --gradle-version 8.7

# Then build:
./gradlew assembleDebug
```

Or: **Android Studio → Open → select the `android/` directory.** Studio will
provision the wrapper and sync.

## Setting the server URL

The server base URL is editable on the **Login screen** (default
`http://127.0.0.1:8096`) and is persisted in `EncryptedSharedPreferences`. It
must be reachable from the device:

- **Emulator:** use `http://10.0.2.2:8096` to reach the host machine's
  `localhost`.
- **Physical device:** use the host's LAN address, e.g. `http://192.168.0.10:8096`.

Cleartext HTTP is permitted via
[`res/xml/network_security_config.xml`](app/src/main/res/xml/network_security_config.xml)
so LAN servers work out of the box. Tighten this for public deployments.

## API calls made by the app

| Purpose      | Method & path                                   |
|--------------|-------------------------------------------------|
| Login        | `POST /api/v1/auth/login`                       |
| Logout       | `POST /api/v1/auth/logout`                      |
| (available)  | `GET /api/v1/auth/me`                           |
| Libraries    | `GET /api/v1/libraries`                         |
| Browse items | `GET /api/v1/items?library_id&q&sort&limit&offset` |
| Item detail  | `GET /api/v1/items/{id}`                        |
| Poster       | `GET /api/v1/items/{id}/artwork?type=poster`    |
| Start play   | `POST /api/v1/playback/start`                   |
| Stop play    | `POST /api/v1/playback/stop`                    |

All non-login requests carry `Authorization: Bearer <token>` via
`AuthInterceptor`.

## Source layout

```
app/src/main/java/dev/flex/app/
├── FlexApp.kt                 # Application, owns AppContainer
├── MainActivity.kt
├── data/
│   ├── ApiClient.kt           # Retrofit + OkHttp factory
│   ├── AppContainer.kt        # tiny manual DI
│   ├── AuthInterceptor.kt     # Authorization: Bearer
│   ├── FlexApi.kt             # Retrofit interface
│   ├── FlexRepository.kt
│   ├── TokenStore.kt          # EncryptedSharedPreferences
│   └── model/Models.kt        # DTOs mirroring flex-protocol
└── ui/
    ├── FlexNavHost.kt
    ├── MainViewModel.kt
    ├── auth/LoginScreen.kt
    ├── library/LibraryScreen.kt
    ├── detail/DetailScreen.kt
    ├── player/PlayerScreen.kt
    └── theme/{Color,Theme,Type}.kt
```

## Unverified / risky

- **Nothing here has been compiled.** The Gradle wrapper jar is absent; AGP,
  Kotlin, Compose, Media3 and library versions are pinned to plausible,
  mutually-compatible releases but were not resolved from a repository.
- **`security-crypto` 1.1.0-alpha06** — the `EncryptedSharedPreferences` API
  used here matches that alpha; Google has since deprecated the library. If a
  newer version is pulled, the API/signatures may differ.
- **kotlinx-serialization converter** — relies on
  `retrofit2-kotlinx-serialization-converter`'s `asConverterFactory` shipped in
  Retrofit 2.11.0. Verify the artifact resolves.
- **Playback auth model** — per `docs/API.md`, playback URLs carry a scoped
  `?t=` token and need no headers. The player *also* attaches the bearer token
  as a default request property for robustness (covers direct-play file URLs);
  if the server rejects unexpected headers on scoped URLs, remove it in
  `PlayerScreen.kt`.
- **Direct-play URL assumption** — the task states the returned URL always
  carries auth; if a v1 server returns a bare `/items/{id}/stream` for
  `direct` mode, ExoPlayer depends on the bearer header above.
- **Cleartext traffic** is enabled globally for LAN convenience; not
  production-hardened.
- **No tests, no lint run.** Iconography uses Material icons; the launcher icon
  is a simple vector.
