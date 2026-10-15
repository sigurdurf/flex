# Flex — Android client

A native Android client for **Flex** (an open-source Plex alternative). It talks
to a `flex-server` over the HTTP API described in
[`docs/API.md`](../docs/API.md), whose wire types live in
[`crates/flex-protocol`](../crates/flex-protocol/src/lib.rs).

> **Built, linted, and packaged** with JDK 17 (Temurin), Gradle 8.7, and Android
> SDK 35. `./gradlew assembleDebug`, `./gradlew assembleRelease` (R8/minified),
> and `./gradlew lintDebug` all pass with **0 lint errors**. See
> [Verification](#verification).

## Features

1. **Login** — server URL (default `http://127.0.0.1:8096`), username, password,
   optional device name. `POST /api/v1/auth/login`; the bearer token is stored in
   `EncryptedSharedPreferences`.
2. **Browse** — library sidebar + adaptive poster grid from `GET /api/v1/items`
   (paginated) with posters from `GET /api/v1/items/{id}/artwork?type=poster`.
   The search box maps to the `q` query parameter.
3. **Continue watching** — a horizontal row above the grid, from
   `GET /api/v1/continue-watching`, showing each in-progress item with a progress
   bar. Tapping a card resumes playback immediately.
4. **Detail** — title, year, container, duration, and video/audio/subtitle track
   listings; the Play button becomes **Resume mm:ss** and a progress bar is shown
   when saved progress exists.
5. **Player** — Media3 `ExoPlayer` in a `PlayerView`. On Play it calls
   `POST /api/v1/playback/start` advertising typical Android decoder
   capabilities, passing `start_position_ms` to resume. The returned `url` (a
   direct file URL or an HLS master playlist carrying `?t=<session_token>`) is
   handed straight to ExoPlayer. Progress is saved every 5 s and on exit via
   `PUT /api/v1/progress/{item_id}`.
6. **Logout** — `POST /api/v1/auth/logout`, clears the stored token, returns to
   login.

## Prerequisites

- **JDK 17** (verified with Temurin 17.0.20.1) and **Android SDK Platform 35**
  plus its **Build-Tools** (install via `sdkmanager`).
- The Gradle wrapper is committed; no separate Gradle install is needed.
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

The Gradle wrapper is committed (`gradlew`, `gradle/wrapper/gradle-wrapper.jar`).
Point the build at your SDK by creating `android/local.properties` (gitignored):

```properties
sdk.dir=/path/to/Android/Sdk
```

Then, from `android/`:

```bash
./gradlew assembleDebug     # debug APK
./gradlew assembleRelease   # R8-minified (unsigned) release APK
./gradlew lintDebug         # Android Lint (currently 0 errors)
```

Or: **Android Studio → Open → select the `android/` directory.**

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
| Continue     | `GET /api/v1/continue-watching`                 |
| Progress     | `GET/PUT/DELETE /api/v1/progress/{item_id}`     |

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

## Verification

Built and checked with JDK 17 (Temurin 17.0.20.1), Gradle 8.7, Android
compileSdk/targetSdk 35, Build-Tools 35.0.0:

| Task                                    | Result                       |
|-----------------------------------------|------------------------------|
| `./gradlew assembleDebug`               | ✅ `app-debug.apk`           |
| `./gradlew assembleRelease` (R8)        | ✅ `app-release-unsigned.apk`|
| `./gradlew lintDebug`                   | ✅ 0 errors (37 warnings)    |

The build was **not run on a device/emulator** here, so runtime behaviour
(login against a live server, actual playback) is unverified.

### Known limitations / notes

- **`security-crypto` 1.1.0-alpha06** — `EncryptedSharedPreferences` is now
  deprecated by Google; pinning the alpha keeps the API stable. Migrate to
  `DataStore` + your own keystore wrapping eventually.
- **Playback auth model** — playback URLs carry a scoped `?t=` token and need no
  headers. The player *also* attaches the bearer token as a default request
  property for robustness (covers bare direct-play file URLs).
- **Direct-play resume** — the server only applies resume server-side for
  remux/transcode; for direct play the client passes `start_position_ms` and
  seeks, which is why the resume position is threaded through the ViewModel.
- **No unit/instrumented tests** yet. Iconography uses Material icons; the
  launcher icon is a simple vector.
- **Cleartext traffic** is enabled via `network_security_config.xml` for LAN
  convenience; tighten for public deployments.
