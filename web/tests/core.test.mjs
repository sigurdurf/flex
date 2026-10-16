import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { after, before, test } from "node:test";
import {
  ApiClient, ArtworkCache, browserCapabilities, formatEpisodeNumber, formatRuntime, formatTime,
  normalizeServer, progressPayload, readStorage, seriesCounts, writeStorage
} from "../core.mjs";

let server;
let base;
const requests = [];
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jVJkAAAAASUVORK5CYII=", "base64");

before(async () => {
  server = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    requests.push({ path: request.url, method: request.method, auth: request.headers.authorization, body });
    const path = request.url.split("?")[0];
    if (path === "/api/v1/slow") return;
    if (path === "/api/v1/slow-body") {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.write('{"items":');
      return;
    }
    if (path === "/api/v1/not-json") {
      response.writeHead(200, { "Content-Type": "text/html" }).end("<html>Wrong address</html>");
      return;
    }
    if (path === "/poster.png") {
      response.writeHead(200, { "Content-Type": "image/png" }).end(png);
      return;
    }
    if (path === "/api/v1/empty") { response.writeHead(204).end(); return; }
    response.setHeader("Content-Type", "application/json");
    if (path === "/api/v1/invalid-json") { response.end("{broken"); return; }
    if (path === "/api/v1/unauthorized") { response.writeHead(401).end("{}"); return; }
    if (path === "/api/v1/missing" || path === "/missing.png") { response.writeHead(404).end("{}"); return; }
    if (path === "/api/v1/rate-limited") { response.writeHead(429).end("{}"); return; }
    if (path === "/api/v1/error") { response.writeHead(502).end(JSON.stringify({ error: "upstream", message: "Subtitle provider unavailable" })); return; }
    response.end(JSON.stringify({ items: [], total: 0, body: body ? JSON.parse(body) : null }));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  base = `http://127.0.0.1:${server.address().port}`;
});

after(() => { server.closeAllConnections(); server.close(); });

test("normalizes origins, local hostnames, and reverse-proxy prefixes", () => {
  assert.equal(normalizeServer("", "http://localhost:8096"), "http://localhost:8096");
  assert.equal(normalizeServer(" localhost:8096/// "), "http://localhost:8096");
  assert.equal(normalizeServer("https://media.example/flex/"), "https://media.example/flex");
  assert.equal(normalizeServer("http://[::1]:8096/"), "http://[::1]:8096");
});

test("rejects unsupported protocols, embedded credentials, queries, and malformed addresses", () => {
  for (const value of ["ftp://media.example", "file:///tmp", "https://user:pass@media.example", "https://media.example/?token=secret", "https://media.example/#part", "http://", "bad address"]) {
    assert.throws(() => normalizeServer(value), Error, value);
  }
});

test("browsing sends bearer auth and preserves filters", async () => {
  const client = new ApiClient(base, "test-device-token");
  assert.deepEqual(await client.request("/items?q=river&sort=-year"), { items: [], total: 0, body: null });
  const request = requests.at(-1);
  assert.equal(request.path, "/api/v1/items?q=river&sort=-year");
  assert.equal(request.auth, "Bearer test-device-token");
  assert.equal(request.method, "GET");
});

test("playback sends an explicit zero when playing from the beginning", async () => {
  await new ApiClient(base, "test-device-token").request("/playback/start", {
    method: "POST", body: { item_id: 7, start_position_ms: 0 }
  });
  assert.deepEqual(JSON.parse(requests.at(-1).body), { item_id: 7, start_position_ms: 0 });
});

test("logout accepts an empty 204 response", async () => {
  assert.equal(await new ApiClient(base).request("/empty", { method: "POST" }), null);
});

test("authenticated 401 expires the session, but a rejected login does not", async () => {
  let expired = 0;
  const options = { onUnauthorized: () => expired++ };
  await assert.rejects(new ApiClient(base, "expired", options).request("/unauthorized"), { status: 401 });
  assert.equal(expired, 1);
  await assert.rejects(new ApiClient(base, "", options).request("/unauthorized"), /username or password/);
  assert.equal(expired, 1);
});

test("keeps server error messages and gives readable rate-limit errors", async () => {
  const client = new ApiClient(base);
  await assert.rejects(client.request("/error"), { status: 502, message: "Subtitle provider unavailable" });
  await assert.rejects(client.request("/rate-limited"), { status: 429, message: "Too many requests. Wait a moment, then try again." });
});

test("rejects SPA HTML and malformed JSON instead of presenting an empty library", async () => {
  const client = new ApiClient(base);
  await assert.rejects(client.request("/not-json"), /did not return JSON/);
  await assert.rejects(client.request("/invalid-json"), /unreadable response/);
});

test("times out a server that never sends headers", async () => {
  await assert.rejects(new ApiClient(base, "", { timeoutMs: 30 }).request("/slow"), { code: "timeout" });
});

test("times out a response whose JSON body stalls", async () => {
  await assert.rejects(new ApiClient(base, "", { timeoutMs: 30 }).request("/slow-body"), { code: "timeout" });
});

test("navigation can cancel a pending request without expiring the session", async () => {
  const controller = new AbortController();
  let expired = false;
  const promise = new ApiClient(base, "valid", { onUnauthorized: () => { expired = true; } }).request("/slow", { signal: controller.signal });
  controller.abort();
  await assert.rejects(promise, { name: "AbortError" });
  assert.equal(expired, false);
});

test("an already-aborted request makes no network request", async () => {
  const controller = new AbortController();
  controller.abort();
  const beforeCount = requests.length;
  await assert.rejects(new ApiClient(base).request("/items", { signal: controller.signal }), { name: "AbortError" });
  assert.equal(requests.length, beforeCount);
});

test("never forwards bearer credentials to an external artwork or playback host", async () => {
  await assert.rejects(new ApiClient(base, "private").fetch("https://other.example/poster.png"), /different host/);
  assert.throws(() => new ApiClient(base).url("https://other.example/stream"), /different host/);
});

test("missing poster metadata does not trigger pointless artwork requests", async () => {
  const beforeCount = requests.length;
  const cache = new ArtworkCache(new ApiClient(base));
  assert.equal(await cache.load({ id: 1, poster_url: null }), null);
  assert.equal(requests.length, beforeCount);
  cache.dispose();
});

test("deduplicates artwork requests and revokes the resulting blob on disposal", async () => {
  const cache = new ArtworkCache(new ApiClient(base, "valid"));
  const item = { id: 1, poster_url: "/poster.png" };
  const beforeCount = requests.filter(request => request.path === "/poster.png").length;
  const [first, second] = await Promise.all([cache.load(item), cache.load(item)]);
  assert.equal(first, second);
  assert.equal(requests.filter(request => request.path === "/poster.png").length, beforeCount + 1);
  assert.equal((await fetch(first)).status, 200);
  cache.dispose();
  await assert.rejects(fetch(first));
  assert.equal(await cache.load(item), null);
});

test("caches missing artwork and can dispose an in-flight request", async () => {
  const cache = new ArtworkCache(new ApiClient(base));
  const item = { id: 1, poster_url: "/missing.png" };
  const beforeCount = requests.filter(request => request.path === "/missing.png").length;
  assert.equal(await cache.load(item), null);
  assert.equal(await cache.load(item), null);
  assert.equal(requests.filter(request => request.path === "/missing.png").length, beforeCount + 1);
  const pending = cache.load({ id: 2, poster_url: "/api/v1/slow" });
  cache.dispose();
  assert.equal(await pending, null);
});

test("never replaces saved progress before metadata and the initial seek are ready", () => {
  assert.equal(progressPayload({ ready: false, durationMs: 120000 }, 0, NaN), null);
  assert.equal(progressPayload({ ready: true }, NaN, NaN), null);
});

test("direct playback uses the file position without adding a resume offset", () => {
  assert.deepEqual(progressPayload({ ready: true, durationMs: 120000, offsetMs: 0 }, 45, 120), { position_ms: 45000, duration_ms: 120000 });
});

test("HLS progress adds the server seek offset and keeps the original duration", () => {
  assert.deepEqual(progressPayload({ ready: true, durationMs: 120000, offsetMs: 30000 }, 15, 90), { position_ms: 45000, duration_ms: 120000 });
});

test("HLS can derive total duration and marks a completed video watched", () => {
  assert.deepEqual(progressPayload({ ready: true, offsetMs: 30000 }, 90, 90, true), { position_ms: 120000, duration_ms: 120000, watched: true });
});

test("clamps progress to duration and accepts unknown durations", () => {
  assert.deepEqual(progressPayload({ ready: true, durationMs: 60000 }, 65, 60), { position_ms: 60000, duration_ms: 60000 });
  assert.deepEqual(progressPayload({ ready: true }, 3, Infinity), { position_ms: 3000 });
});

test("only advertises codecs and HLS actually supported by the browser", () => {
  const supports = { canPlayType: type => type.includes("avc1") || type.includes("mp4a") ? "probably" : "" };
  const capabilities = browserCapabilities(supports, { isSupported: () => true });
  assert.deepEqual(capabilities.video_codecs, ["h264"]);
  assert.deepEqual(capabilities.audio_codecs, ["aac"]);
  assert.deepEqual(capabilities.containers, ["mp4", "mov", "m4v"]);
  assert.equal(capabilities.hls, true);
  assert.equal(browserCapabilities({ canPlayType: () => "" }).hls, false);
});

test("blocked localStorage does not prevent sign-in or browsing", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw new Error("Storage denied"); } });
  try {
    assert.equal(readStorage("flex.server", "fallback"), "fallback");
    assert.equal(writeStorage("flex.token:server", "token"), false);
    assert.equal(writeStorage("flex.token:server", null), false);
  } finally {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  }
});

test("formats resume times and runtime without hiding zero", () => {
  assert.equal(formatTime(0), "0:00");
  assert.equal(formatTime(3723000), "1:02:03");
  assert.equal(formatRuntime(5400000), "1h 30m");
  assert.equal(formatRuntime(null), "");
});

test("pluralizes season and episode counts for series cards", () => {
  assert.deepEqual(seriesCounts({ season_count: 1, episode_count: 1 }), { seasons: "1 season", episodes: "1 episode" });
  assert.deepEqual(seriesCounts({ season_count: 2, episode_count: 10 }), { seasons: "2 seasons", episodes: "10 episodes" });
  assert.deepEqual(seriesCounts({}), { seasons: "0 seasons", episodes: "0 episodes" });
  assert.deepEqual(seriesCounts(null), { seasons: "0 seasons", episodes: "0 episodes" });
});

test("labels episode numbers with zero-padding and a placeholder", () => {
  assert.equal(formatEpisodeNumber(1), "E01");
  assert.equal(formatEpisodeNumber(10), "E10");
  assert.equal(formatEpisodeNumber(105), "E105");
  assert.equal(formatEpisodeNumber(0), "—");
  assert.equal(formatEpisodeNumber(null), "—");
});
