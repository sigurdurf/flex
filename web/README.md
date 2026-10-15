# Flex web client

Browse and play media from a Flex server. The client uses HTML, CSS, and
JavaScript modules with no build step, package manager, or external font and
image services. The only third-party runtime dependency is the vendored
[hls.js](vendor/hls.js) player.

Open `http://<host>:<port>/` after starting `flex-server`. Sign in with an account
created on that server. Connection settings default to the current origin and
can be expanded to choose another server or device name.

## Using the library

- Select a library in the sidebar, or use All media to browse the collection.
  On small screens, the menu button opens library navigation and sign-out.
- Search titles, sort by title, year, or date added, and switch between grid and
  list views. Search, sort, and library selection stay in the URL. The display
  preference stays in browser storage.
- Open a title for its description, file information, audio tracks, subtitles,
  and playback controls. Browser Back returns to the library without discarding
  its loaded pages. Titles without artwork have a readable fallback.
- Continue watching resumes saved progress. Play from beginning explicitly
  starts at zero. The player uses native video controls and closes with Escape
  or Close player.
- Find subtitles searches providers configured on the server and downloads a
  chosen result as a sidecar. Track information is shown in the detail page;
  audio and subtitle track selection in the web player is not implemented.
- Scan media scans every configured library, then reloads the index and watch
  list. The button stays disabled during the scan and reports added, updated,
  and removed files, along with any scan errors.
  Failed refreshes keep existing titles visible. Each failed section has a
  retry control. Empty searches and empty libraries have separate messages.

The client requires a current browser with JavaScript modules, `fetch`,
`AbortController`, and native `<dialog>` support. Playback support depends on
its codecs, native HLS support, and Media Source Extensions.

## Files

- `index.html` contains the login, navigation, library, detail, and player UI.
- `styles.css` contains the responsive layout and focus states. Reduced motion
  preferences disable transitions.
- `app.js` coordinates navigation, section loading, authentication, and playback.
- `core.mjs` contains the API client, storage helpers, artwork cache, browser
  capability detection, and progress calculations.
- `icon.svg` is the local favicon.
- `vendor/hls.js` provides HLS playback when the browser lacks native HLS.
- `tests/core.test.mjs` tests the client against a temporary HTTP server with
  Node's built-in test runner.

## Requests and storage

The server serves this directory at `/`. Requests use `<server>/api/v1/...`
with a bearer token. Each section cancels its older read request when replaced
and checks its session before applying results. Requests time out after 20
seconds, except media scans, which wait until the scan completes.
An authenticated `401` clears the token and returns to sign-in;
network failures leave the session intact.

Browser storage keys are:

- `flex.server` for the server URL.
- `flex.token:<server-url>` for its device token.
- `flex.view` for grid or list mode.

Blocked storage does not prevent sign-in. The client keeps the session in
memory and explains that a later visit will require signing in again. Passwords
are cleared after successful sign-in.

Artwork uses authenticated requests and blob URLs. Requests are shared between
cards and details and start near the viewport. Signing out or changing sessions
cancels artwork requests and revokes blob URLs. Artwork and playback URLs must
use the selected server's origin.

## Playback and progress

The client reports detected browser capabilities to `/playback/start`. Direct
files seek to the resume position after metadata loads. HLS sessions start at
that position on the server, so saved progress adds the seek offset back to the
video's elapsed time. The original duration remains the progress denominator.
The server includes scoped tokens on playlist segments and initialization URLs
for native HLS players.

Progress is saved every 10 seconds, on pause and completion, and when the page
becomes hidden. Writes run in order and coalesce pending positions on slow
connections. Failed writes produce a message inside the player. Close and page
exit attempt to save progress and stop the playback session with `keepalive`.
As with other browser exit requests, delivery is best effort.

Closing a player while its start request is pending also stops the session if
it arrives later. HLS recovery attempts are bounded. Playback errors stay in the
player with a retry action.

## Verification

Run with Node 22 or newer. No install step is needed:

```sh
node --check web/app.js
node --check web/core.mjs
node --test web/tests/*.test.mjs
```

CI runs these checks in the `web` job. Server playlist authorization and byte
range tests run with `cargo test -p flex-server`.

For browser checks, run an isolated Flex server with sample media and verify
sign-in, library selection, search, sorting, pagination, Back and Forward,
resume, playback, and sign-out at desktop and phone widths. Check failed reads
and retries as well as successful requests. Native HLS and hls.js playback
need separate checks because their media loaders differ.
