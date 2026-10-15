// Browser-independent pieces are also exercised by the Node test suite.
export function normalizeServer(value, origin) {
  let address = String(value || origin || "").trim();
  if (!/^[a-z][a-z\d+.-]*:\/\//i.test(address)) address = "http://" + address;
  let url;
  try { url = new URL(address); } catch {
    throw new Error("Enter a valid server address, such as http://localhost:8096.");
  }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
    throw new Error("Use an HTTP or HTTPS server address without credentials, a query, or a fragment.");
  }
  return url.href.replace(/\/+$/, "");
}

export function readStorage(key, fallback = "") {
  try { return globalThis.localStorage.getItem(key) ?? fallback; } catch { return fallback; }
}

export function writeStorage(key, value) {
  try {
    if (value == null) globalThis.localStorage.removeItem(key);
    else globalThis.localStorage.setItem(key, value);
    return true;
  } catch { return false; }
}

export function formatTime(ms) {
  const seconds = Math.max(0, Math.floor((Number(ms) || 0) / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds % 3600 / 60);
  const rest = String(seconds % 60).padStart(2, "0");
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
}

export function formatRuntime(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const minutes = Math.max(1, Math.round(ms / 60000));
  return minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${minutes}m`;
}

export function progressPayload(playback, currentTime, duration, ended = false) {
  // Metadata and any initial seek must finish before a zero can overwrite a resume point.
  if (!playback?.ready || !Number.isFinite(currentTime) || currentTime < 0) return null;
  const offset = playback.offsetMs || 0;
  const total = playback.durationMs || (Number.isFinite(duration) && duration > 0 ? Math.round(duration * 1000) + offset : 0);
  let position = Math.max(0, Math.round(currentTime * 1000) + offset);
  if (total) position = Math.min(position, total);
  const payload = { position_ms: position };
  if (total) payload.duration_ms = total;
  if (ended) payload.watched = true;
  return payload;
}

export class ApiError extends Error {
  constructor(message, status = 0, code = "") {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

export class ApiClient {
  constructor(server, token = "", { onUnauthorized = () => {}, timeoutMs = 20000 } = {}) {
    this.server = server;
    this.token = token;
    this.onUnauthorized = onUnauthorized;
    this.timeoutMs = timeoutMs;
  }

  url(path) {
    const url = new URL(/^https?:\/\//i.test(path) ? path : this.server + (path.startsWith("/") ? path : "/" + path));
    // Artwork and session URLs must never send a user's bearer token to another host.
    if (url.origin !== new URL(this.server).origin) throw new ApiError("The server returned a URL for a different host.");
    return url.href;
  }

  request(path, options = {}) {
    return this.fetch("/api/v1" + path, options);
  }

  async fetch(path, { method = "GET", body, signal, keepalive = false, responseType = "json", timeoutMs = this.timeoutMs } = {}) {
    const controller = new AbortController();
    const abort = () => controller.abort();
    let timedOut = false;
    if (signal?.aborted) controller.abort();
    else signal?.addEventListener("abort", abort, { once: true });
    const timeout = timeoutMs > 0 ? setTimeout(() => { timedOut = true; controller.abort(); }, timeoutMs) : null;
    const headers = {};
    if (this.token) headers.Authorization = "Bearer " + this.token;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    try {
      const response = await fetch(this.url(path), {
        method, headers, signal: controller.signal, keepalive,
        cache: responseType === "blob" ? "default" : "no-store",
        ...(body !== undefined ? { body: JSON.stringify(body) } : {})
      });
      if (!response.ok) {
        if (response.status === 401 && this.token) this.onUnauthorized();
        const data = await response.json().catch(() => null);
        const fallback = {
          401: this.token ? "Your session expired. Sign in again." : "The username or password is incorrect.",
          403: "Your account does not have access to this action.",
          404: "This item is no longer available.",
          429: "Too many requests. Wait a moment, then try again.",
          503: "The server is busy or this service is unavailable. Try again shortly."
        }[response.status] || `The server returned an error (${response.status}).`;
        throw new ApiError(typeof data?.message === "string" ? data.message : fallback, response.status, data?.error);
      }
      if (response.status === 204) return null;
      if (responseType === "blob") return await response.blob();
      const type = response.headers.get("content-type") || "";
      if (!type.includes("application/json")) throw new ApiError("The server did not return JSON. Check the server address.");
      try { return await response.json(); } catch (error) {
        if (controller.signal.aborted) throw error;
        throw new ApiError("The server returned an unreadable response. Try again.");
      }
    } catch (error) {
      if (timedOut) throw new ApiError("The server took too long to respond. Try again.", 0, "timeout");
      if (controller.signal.aborted) throw new DOMException("Request cancelled", "AbortError");
      if (error instanceof ApiError) throw error;
      throw new ApiError("Couldn't reach the server. Check your connection and server address.", 0, "network");
    } finally {
      if (timeout !== null) clearTimeout(timeout);
      signal?.removeEventListener("abort", abort);
    }
  }
}

export class ArtworkCache {
  constructor(client) {
    this.client = client;
    this.entries = new Map();
    this.urls = new Set();
    this.controller = new AbortController();
    this.disposed = false;
  }

  load(item) {
    if (!item.poster_url || this.disposed) return Promise.resolve(null);
    if (this.entries.has(item.poster_url)) return this.entries.get(item.poster_url);
    const promise = this.client.fetch(item.poster_url, { responseType: "blob", signal: this.controller.signal }).then(blob => {
      if (this.disposed || !blob.type.startsWith("image/")) return null;
      const url = URL.createObjectURL(blob);
      this.urls.add(url);
      return url;
    }).catch(error => {
      if (error.status !== 404) this.entries.delete(item.poster_url);
      return null;
    });
    this.entries.set(item.poster_url, promise);
    return promise;
  }

  dispose() {
    this.disposed = true;
    this.controller.abort();
    for (const url of this.urls) URL.revokeObjectURL(url);
    this.urls.clear();
    this.entries.clear();
  }
}

export function browserCapabilities(video, Hls) {
  const supports = type => !!video.canPlayType(type);
  const mp4 = supports('video/mp4; codecs="avc1.42E01E"');
  const webm = supports('video/webm; codecs="vp9"');
  return {
    containers: [...(mp4 ? ["mp4", "mov", "m4v"] : []), ...(webm ? ["webm"] : [])],
    video_codecs: [
      ...(mp4 ? ["h264"] : []),
      ...(webm ? ["vp9"] : []),
      ...(supports('video/webm; codecs="vp8"') ? ["vp8"] : []),
      ...(supports('video/mp4; codecs="av01.0.04M.08"') ? ["av1"] : []),
      ...(supports('video/mp4; codecs="hvc1.1.6.L93.B0"') ? ["hevc"] : [])
    ],
    audio_codecs: [
      ["aac", 'audio/mp4; codecs="mp4a.40.2"'], ["opus", 'audio/webm; codecs="opus"'],
      ["vorbis", 'audio/webm; codecs="vorbis"'], ["mp3", "audio/mpeg"], ["flac", "audio/flac"]
    ].filter(([, type]) => supports(type)).map(([codec]) => codec),
    hls: supports("application/vnd.apple.mpegurl") || !!Hls?.isSupported(),
    hdr: false
  };
}
