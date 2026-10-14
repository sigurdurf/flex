(function () {
  "use strict";

  var API_BASE = "/api/v1";
  var LS_SERVER = "flex.server";
  var TOKEN_PREFIX = "flex.token:";

  var CAPABILITIES = {
    containers: ["mp4", "webm"],
    video_codecs: ["h264", "vp9", "av1"],
    audio_codecs: ["aac", "opus", "vorbis", "mp3", "flac"],
    hls: true
  };

  var state = {
    server: "",
    token: "",
    user: null,
    libraries: [],
    libraryId: null,
    query: "",
    items: [],
    offset: 0,
    limit: 48,
    total: 0,
    loading: false,
    browseSeq: 0,
    currentItem: null,
    currentProgress: null,
    artwork: {},
    video: null,
    hls: null,
    playback: null,
    progressItemId: null,
    progressTimer: null
  };

  function $(id) { return document.getElementById(id); }

  function esc(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function normServer(value) {
    var s = (value || "").trim();
    if (!s) s = location.origin;
    if (!/^https?:\/\//i.test(s)) s = "http://" + s;
    return s.replace(/\/+$/, "");
  }

  function tokenKey(server) {
    return TOKEN_PREFIX + server;
  }

  function absolute(path) {
    if (/^https?:\/\//i.test(path)) return path;
    return state.server + (path.charAt(0) === "/" ? path : "/" + path);
  }

  function toast(message) {
    var el = $("toast");
    el.textContent = message;
    el.hidden = false;
    clearTimeout(toast._t);
    toast._t = setTimeout(function () { el.hidden = true; }, 3200);
  }

  function formatDuration(ms) {
    if (!ms || ms <= 0) return "";
    var total = Math.floor(ms / 1000);
    var h = Math.floor(total / 3600);
    var m = Math.floor((total % 3600) / 60);
    var s = total % 60;
    function pad(n) { return n < 10 ? "0" + n : "" + n; }
    return h > 0 ? h + ":" + pad(m) + ":" + pad(s) : m + ":" + pad(s);
  }

  function api(path, opts) {
    opts = opts || {};
    var headers = Object.assign({}, opts.headers || {});
    if (state.token) headers["Authorization"] = "Bearer " + state.token;
    var init = { method: opts.method || "GET", headers: headers };
    if (opts.keepalive) init.keepalive = true;
    if (opts.body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = typeof opts.body === "string" ? opts.body : JSON.stringify(opts.body);
    }
    return fetch(absolute(API_BASE + path), init).then(function (res) {
      if (res.status === 401) {
        handleUnauthorized();
        throw new Error("unauthorized");
      }
      if (!res.ok) {
        return res.json().catch(function () { return null; }).then(function (body) {
          var msg = (body && (body.message || body.error)) || ("HTTP " + res.status);
          throw new Error(msg);
        });
      }
      if (res.status === 204) return null;
      var ct = res.headers.get("content-type") || "";
      if (ct.indexOf("application/json") !== -1) return res.json();
      return res.text();
    });
  }

  function handleUnauthorized() {
    if (state.token) {
      try { localStorage.removeItem(tokenKey(state.server)); } catch (e) {}
    }
    state.token = "";
    state.user = null;
    showLogin("Your session expired. Please sign in again.");
  }

  function loadArtwork(item) {
    if (state.artwork[item.id]) return Promise.resolve(state.artwork[item.id]);
    var path = item.poster_url || (API_BASE + "/items/" + item.id + "/artwork?type=poster");
    return fetch(absolute(path), { headers: { Authorization: "Bearer " + state.token } })
      .then(function (res) {
        if (!res.ok) throw new Error("no artwork");
        return res.blob();
      })
      .then(function (blob) {
        var url = URL.createObjectURL(blob);
        state.artwork[item.id] = url;
        return url;
      })
      .catch(function () { return null; });
  }

  function showLogin(message) {
    $("login-view").hidden = false;
    $("app-view").hidden = true;
    closePlayer(true);
    var err = $("login-error");
    if (message) { err.textContent = message; err.hidden = false; }
    else { err.hidden = true; }
  }

  function showApp() {
    $("login-view").hidden = true;
    $("app-view").hidden = false;
    $("user-label").textContent = state.user ? state.user.username : "";
  }

  function doLogin(event) {
    event.preventDefault();
    var server = normServer($("server-input").value);
    var username = $("username-input").value.trim();
    var password = $("password-input").value;
    var deviceName = $("device-input").value.trim() || "Web browser";
    if (!username || !password) return;

    state.server = server;
    state.token = "";
    try { localStorage.setItem(LS_SERVER, server); } catch (e) {}

    var btn = $("login-submit");
    btn.disabled = true;
    $("login-error").hidden = true;

    fetch(absolute(API_BASE + "/auth/login"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: username, password: password, device_name: deviceName })
    }).then(function (res) {
      if (!res.ok) {
        return res.json().catch(function () { return null; }).then(function (body) {
          var msg = (body && (body.message || body.error)) || (res.status === 401 ? "Invalid credentials" : "Login failed");
          throw new Error(msg);
        });
      }
      return res.json();
    }).then(function (data) {
      state.token = data.token;
      state.user = data.user;
      try { localStorage.setItem(tokenKey(server), data.token); } catch (e) {}
      showApp();
      return bootstrapData();
    }).catch(function (err) {
      var el = $("login-error");
      el.textContent = err.message || "Login failed";
      el.hidden = false;
    }).then(function () {
      btn.disabled = false;
    });
  }

  function doLogout() {
    api("/auth/logout", { method: "POST" }).catch(function () {}).then(function () {
      try { localStorage.removeItem(tokenKey(state.server)); } catch (e) {}
      state.token = "";
      state.user = null;
      state.libraries = [];
      state.items = [];
      state.artwork = {};
      renderLibraries();
      showLogin();
    });
  }

  function bootstrapData() {
    return Promise.all([
      api("/libraries").catch(function () { return []; }),
      api("/items?limit=" + state.limit + "&offset=0").catch(function () { return null; })
    ]).then(function (results) {
      state.libraries = results[0] || [];
      renderLibraries();
      if (results[1]) {
        state.items = results[1].items || [];
        state.total = results[1].total || 0;
        state.offset = state.items.length;
      }
      renderGrid();
      return loadContinueWatching();
    });
  }

  function renderLibraries() {
    var nav = $("libraries");
    nav.innerHTML = "";
    var all = document.createElement("button");
    all.type = "button";
    all.className = "lib-btn" + (state.libraryId == null ? " active" : "");
    all.textContent = "All";
    all.onclick = function () { selectLibrary(null); };
    nav.appendChild(all);
    state.libraries.forEach(function (lib) {
      var b = document.createElement("button");
      b.type = "button";
      b.className = "lib-btn" + (String(state.libraryId) === String(lib.id) ? " active" : "");
      b.textContent = lib.name;
      b.onclick = function () { selectLibrary(lib.id); };
      nav.appendChild(b);
    });
  }

  function selectLibrary(id) {
    state.libraryId = id;
    state.query = "";
    $("search-input").value = "";
    renderLibraries();
    reloadItems();
  }

  function browseParams(offset) {
    var params = ["limit=" + state.limit, "offset=" + offset];
    if (state.libraryId != null) params.push("library_id=" + encodeURIComponent(state.libraryId));
    if (state.query) params.push("q=" + encodeURIComponent(state.query));
    return params.join("&");
  }

  function reloadItems() {
    state.items = [];
    state.offset = 0;
    state.total = 0;
    renderGrid();
    $("status").textContent = "Loading…";
    var seq = ++state.browseSeq;
    api("/items?" + browseParams(0)).then(function (page) {
      if (seq !== state.browseSeq) return;
      state.items = page.items || [];
      state.total = page.total || 0;
      state.offset = state.items.length;
      $("status").textContent = state.items.length ? "" : "No items found.";
      renderGrid();
    }).catch(function (err) {
      if (seq !== state.browseSeq) return;
      if (err.message !== "unauthorized") $("status").textContent = "Failed to load items: " + err.message;
    });
  }

  function loadMore() {
    if (state.loading || state.offset >= state.total) return;
    state.loading = true;
    var seq = state.browseSeq;
    api("/items?" + browseParams(state.offset)).then(function (page) {
      if (seq !== state.browseSeq) return;
      state.items = state.items.concat(page.items || []);
      state.offset = state.items.length;
      state.total = page.total || state.total;
      renderGrid();
    }).catch(function (err) {
      if (err.message !== "unauthorized") toast("Failed to load more: " + err.message);
    }).then(function () { state.loading = false; });
  }

  function makePoster() {
    var wrap = document.createElement("div");
    wrap.className = "poster";
    var fallback = document.createElement("div");
    fallback.className = "poster-fallback";
    var img = document.createElement("img");
    img.alt = "";
    img.hidden = true;
    img.onload = function () { img.hidden = false; fallback.hidden = true; };
    wrap.appendChild(fallback);
    wrap.appendChild(img);
    return { wrap: wrap, img: img };
  }

  function renderGrid() {
    var grid = $("grid");
    grid.innerHTML = "";
    state.items.forEach(function (item) {
      var card = document.createElement("button");
      card.type = "button";
      card.className = "card";
      var poster = makePoster();
      var title = document.createElement("div");
      title.className = "card-title";
      title.textContent = item.title;
      var sub = document.createElement("div");
      sub.className = "card-sub";
      sub.textContent = item.year ? String(item.year) : "";
      card.appendChild(poster.wrap);
      card.appendChild(title);
      card.appendChild(sub);
      card.onclick = function () { openDetail(item.id); };
      grid.appendChild(card);
      loadArtwork(item).then(function (url) { if (url) poster.img.src = url; });
    });
    var more = $("load-more");
    more.hidden = !(state.items.length > 0 && state.offset < state.total);
  }

  function loadContinueWatching() {
    return api("/continue-watching?limit=20").then(function (page) {
      var entries = (page && page.items) || [];
      var section = $("continue-section");
      var row = $("continue-row");
      row.innerHTML = "";
      if (!entries.length) { section.hidden = true; return; }
      section.hidden = false;
      entries.forEach(function (entry) {
        var item = entry.item;
        var watch = entry.state || {};
        var card = document.createElement("button");
        card.type = "button";
        card.className = "continue-card";
        var poster = makePoster();
        var progress = document.createElement("div");
        progress.className = "progress";
        var bar = document.createElement("span");
        var pct = watch.duration_ms ? Math.min(100, (watch.position_ms / watch.duration_ms) * 100) : 0;
        bar.style.width = pct + "%";
        progress.appendChild(bar);
        var body = document.createElement("div");
        body.className = "cc-body";
        var t = document.createElement("div");
        t.className = "cc-title";
        t.textContent = item.title;
        var s = document.createElement("div");
        s.className = "card-sub";
        s.textContent = formatDuration(watch.position_ms) + (watch.duration_ms ? " / " + formatDuration(watch.duration_ms) : "");
        body.appendChild(t);
        body.appendChild(s);
        card.appendChild(poster.wrap);
        card.appendChild(progress);
        card.appendChild(body);
        card.onclick = function () { startPlayback(item, watch.position_ms || 0); };
        row.appendChild(card);
        loadArtwork(item).then(function (url) { if (url) poster.img.src = url; });
      });
    }).catch(function () { $("continue-section").hidden = true; });
  }

  function openDetail(id) {
    $("browse-section").hidden = true;
    $("detail-view").hidden = false;
    $("detail-title").textContent = "Loading…";
    $("detail-meta").textContent = "";
    $("detail-overview").textContent = "";
    $("detail-audio").innerHTML = "";
    $("detail-subs").innerHTML = "";
    $("sub-search-results").innerHTML = "";
    $("detail-resume").hidden = true;
    $("detail-poster").removeAttribute("src");

    Promise.all([
      api("/items/" + id),
      api("/progress/" + id).catch(function () { return null; })
    ]).then(function (results) {
      var item = results[0];
      var progress = results[1];
      state.currentItem = item;
      state.currentProgress = progress;
      renderDetail(item, progress);
    }).catch(function (err) {
      if (err.message !== "unauthorized") {
        $("detail-title").textContent = "Failed to load item";
        $("detail-overview").textContent = err.message;
      }
    });
  }

  function renderDetail(item, progress) {
    $("detail-title").textContent = item.title;
    var meta = [];
    if (item.year) meta.push(item.year);
    if (item.kind) meta.push(item.kind);
    if (item.duration_ms) meta.push(formatDuration(item.duration_ms));
    if (item.container) meta.push(item.container);
    if (item.video) meta.push(item.video.width + "×" + item.video.height + " " + item.video.codec);
    $("detail-meta").textContent = meta.join(" · ");
    $("detail-overview").textContent = item.overview || "";

    loadArtwork(item).then(function (url) {
      if (url) $("detail-poster").src = url;
    });

    var audio = $("detail-audio");
    (item.audio || []).forEach(function (a) {
      var li = document.createElement("li");
      var label = a.title || a.language || "Track " + a.index;
      li.textContent = label + " — " + a.codec + (a.channels ? " " + a.channels + "ch" : "");
      audio.appendChild(li);
    });
    if (!item.audio || !item.audio.length) {
      audio.innerHTML = '<li class="muted">None</li>';
    }

    var subs = $("detail-subs");
    (item.subtitles || []).forEach(function (s) {
      var li = document.createElement("li");
      var label = s.title || s.language || "Track " + s.index;
      li.textContent = label + " — " + s.codec + (s.forced ? " (forced)" : "");
      subs.appendChild(li);
    });
    if (!item.subtitles || !item.subtitles.length) {
      subs.innerHTML = '<li class="muted">None</li>';
    }

    var resume = $("detail-resume");
    if (progress && progress.position_ms > 0 && !progress.watched) {
      resume.hidden = false;
      resume.textContent = "Resume from " + formatDuration(progress.position_ms);
      resume.onclick = function () { startPlayback(item, progress.position_ms); };
    } else {
      resume.hidden = true;
    }

    $("detail-play").onclick = function () { startPlayback(item, 0); };
    $("sub-search-btn").onclick = function () { searchSubtitles(item); };
  }

  function closeDetail() {
    $("detail-view").hidden = true;
    $("browse-section").hidden = false;
    state.currentItem = null;
    state.currentProgress = null;
  }

  function searchSubtitles(item) {
    var list = $("sub-search-results");
    list.innerHTML = "";
    var li = document.createElement("li");
    li.className = "muted";
    li.textContent = "Searching…";
    list.appendChild(li);
    api("/subtitles/search?item_id=" + encodeURIComponent(item.id)).then(function (results) {
      list.innerHTML = "";
      if (!results || !results.length) {
        li = document.createElement("li");
        li.className = "muted";
        li.textContent = "No subtitles found.";
        list.appendChild(li);
        return;
      }
      results.forEach(function (r) {
        var row = document.createElement("li");
        var label = document.createElement("span");
        label.textContent = r.language + (r.title ? " · " + r.title : "") + (r.release ? " · " + r.release : "");
        var btn = document.createElement("button");
        btn.type = "button";
        btn.className = "ghost";
        btn.textContent = "Download";
        btn.onclick = function () {
          btn.disabled = true;
          api("/subtitles/download", {
            method: "POST",
            body: { provider: r.provider, id: r.id, language: r.language, item_id: item.id }
          }).then(function () {
            toast("Subtitle downloaded");
            openDetail(item.id);
          }).catch(function (err) {
            btn.disabled = false;
            toast("Download failed: " + err.message);
          });
        };
        row.appendChild(label);
        row.appendChild(btn);
        list.appendChild(row);
      });
    }).catch(function (err) {
      if (err.message === "unauthorized") return;
      list.innerHTML = "";
      li = document.createElement("li");
      li.className = "muted";
      li.textContent = "Search failed: " + err.message;
      list.appendChild(li);
    });
  }

  function startPlayback(item, startPositionMs) {
    if (!item) return;
    var body = { item_id: item.id, capabilities: CAPABILITIES };
    if (startPositionMs) body.start_position_ms = startPositionMs;
    var overlay = $("player-overlay");
    $("player-status").textContent = "Starting playback…";
    overlay.hidden = false;
    $("player-title").textContent = item.title;

    api("/playback/start", { method: "POST", body: body }).then(function (session) {
      state.playback = session;
      state.progressItemId = item.id;
      openPlayer(session);
    }).catch(function (err) {
      overlay.hidden = true;
      if (err.message !== "unauthorized") toast("Playback failed: " + err.message);
    });
  }

  function openPlayer(session) {
    var video = state.video || $("player-video");
    state.video = video;
    var url = absolute(session.url);
    $("player-status").textContent = "";

    function onError() {
      $("player-status").textContent = "Playback error — the stream could not be played.";
    }
    video.onerror = onError;

    if (state.hls) { state.hls.destroy(); state.hls = null; }

    if (session.stream_kind === "hls" && window.Hls && window.Hls.isSupported()) {
      state.hls = new window.Hls({ enableWorker: true });
      state.hls.on(window.Hls.Events.ERROR, function (event, data) {
        if (data && data.fatal) {
          if (data.type === window.Hls.ErrorTypes.NETWORK_ERROR) state.hls.startLoad();
          else if (data.type === window.Hls.ErrorTypes.MEDIA_ERROR) state.hls.recoverMediaError();
          else onError();
        }
      });
      state.hls.loadSource(url);
      state.hls.attachMedia(video);
    } else {
      video.src = url;
    }

    video.play().catch(function () {});
    startProgressTimer();
  }

  function startProgressTimer() {
    stopProgressTimer();
    state.progressTimer = setInterval(function () { saveProgress(false); }, 10000);
  }

  function stopProgressTimer() {
    if (state.progressTimer) { clearInterval(state.progressTimer); state.progressTimer = null; }
  }

  function saveProgress(keepalive) {
    var video = state.video;
    var itemId = state.progressItemId;
    if (!video || itemId == null) return Promise.resolve();
    if (!isFinite(video.currentTime)) return Promise.resolve();
    var position = Math.round(video.currentTime * 1000);
    var duration = isFinite(video.duration) && video.duration > 0 ? Math.round(video.duration * 1000) : undefined;
    var body = { position_ms: position };
    if (duration) body.duration_ms = duration;
    return api("/progress/" + itemId, { method: "PUT", body: body, keepalive: !!keepalive })
      .catch(function () {});
  }

  function closePlayer(silent) {
    stopProgressTimer();
    var video = state.video;
    var session = state.playback;
    var itemId = state.progressItemId;

    if (!silent && video && itemId != null) {
      saveProgress(true);
      if (session) {
        api("/playback/stop", { method: "POST", body: { session_id: session.session_id }, keepalive: true }).catch(function () {});
      }
    }

    if (state.hls) { state.hls.destroy(); state.hls = null; }
    if (video) {
      try { video.pause(); } catch (e) {}
      video.removeAttribute("src");
      try { video.load(); } catch (e) {}
    }
    state.playback = null;
    state.progressItemId = null;
    $("player-overlay").hidden = true;
    $("player-status").textContent = "";
    if (!silent) loadContinueWatching();
  }

  function bindEvents() {
    $("login-form").addEventListener("submit", doLogin);
    $("logout-btn").addEventListener("click", doLogout);
    $("load-more").addEventListener("click", loadMore);
    $("detail-back").addEventListener("click", closeDetail);
    $("player-close").addEventListener("click", function () { closePlayer(false); });

    var search = $("search-input");
    var searchTimer = null;
    search.addEventListener("input", function () {
      clearTimeout(searchTimer);
      searchTimer = setTimeout(function () {
        state.query = search.value.trim();
        reloadItems();
      }, 300);
    });

    var video = $("player-video");
    state.video = video;
    video.addEventListener("pause", function () { saveProgress(false); });
    video.addEventListener("ended", function () { saveProgress(false); });
    window.addEventListener("beforeunload", function () {
      if (state.progressItemId != null) saveProgress(true);
    });
    document.addEventListener("visibilitychange", function () {
      if (document.hidden && state.progressItemId != null) saveProgress(true);
    });
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && !$("player-overlay").hidden) closePlayer(false);
    });
  }

  function init() {
    state.server = normServer(localStorage.getItem(LS_SERVER) || location.origin);
    $("server-input").value = state.server;
    state.token = localStorage.getItem(tokenKey(state.server)) || "";
    bindEvents();

    if (!state.token) { showLogin(); return; }

    api("/auth/me").then(function (user) {
      state.user = user;
      showApp();
      return bootstrapData();
    }).catch(function (err) {
      if (err.message !== "unauthorized") showLogin("Could not reach the server: " + err.message);
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
