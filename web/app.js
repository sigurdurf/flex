import {
  ApiClient, ArtworkCache, browserCapabilities, formatEpisodeNumber, formatRuntime, formatTime,
  normalizeServer, progressPayload, readStorage, seriesCounts, writeStorage
} from "./core.mjs";

const $ = id => document.getElementById(id);
const SORTS = ["title", "-title", "-added", "-year", "year"];
const state = {
  client: null, artwork: null, user: null, libraries: [], librariesLoaded: false,
  libraryId: null, query: "", sort: "title", view: "grid", items: [], total: 0,
  offset: 0, limit: 48, browseLoaded: false, browseError: null, browseAction: "load", loading: false,
  scanning: false,
  continueEntries: [], continueLoaded: false, continueError: null,
  detailId: null, currentItem: null, progress: null,
  browseMode: "items", seriesId: null, seriesDetail: null, seriesLoaded: false, seriesError: null,
  requests: new Map(), cleanups: new Set(), player: null, playbackAttempt: 0, authBusy: false
};
let searchTimer;
let toastTimer;
let browseScroll = 0;
let browseFocus = null;
let lastHistoryURL = location.href;

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function icon(name) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  node.classList.add("icon");
  node.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(node.namespaceURI, "use");
  use.setAttribute("href", "#icon-" + name);
  node.append(use);
  return node;
}

function button(className, label, onClick) {
  const node = element("button", className, label);
  node.type = "button";
  node.addEventListener("click", onClick);
  return node;
}

function beginRequest(key) {
  state.requests.get(key)?.abort();
  const controller = new AbortController();
  state.requests.set(key, controller);
  return controller;
}

function current(client, controller) {
  return state.client === client && !controller.signal.aborted;
}

function ignoreError(error) {
  return error.name === "AbortError" || error.status === 401;
}

function toast(message) {
  clearTimeout(toastTimer);
  $("toast").textContent = message;
  $("toast").hidden = false;
  toastTimer = setTimeout(() => { $("toast").hidden = true; }, 5500);
}

function tokenKey(server) { return "flex.token:" + server; }
function titleCount(count) { return `${count.toLocaleString()} ${count === 1 ? "title" : "titles"}`; }
function kindLabel(kind) { return { movie: "Movie", episode: "Episode", other: "Video" }[kind] || "Video"; }
function mediaIcon(kind) { return kind === "episode" || kind === "tv" ? "tv" : kind === "movie" || kind === "movies" ? "film" : "folder"; }
function validItem(item) { return item && Number.isSafeInteger(item.id) && typeof item.title === "string"; }
function validSeries(series) { return series && Number.isSafeInteger(series.id) && typeof series.title === "string"; }

// TV libraries browse shows rather than individual episode files.
function activeLibrary() { return state.libraries.find(lib => lib.id === state.libraryId) || null; }
function browseModeOf() { const lib = activeLibrary(); return lib && lib.kind === "tv" ? "series" : "items"; }
function countLabel(count) {
  const noun = state.browseMode === "series" ? "series" : count === 1 ? "title" : "titles";
  return `${count.toLocaleString()} ${noun}`;
}

// Fetch artwork only as it approaches the viewport. The cache shares requests
// between cards and details, and releases every object URL at session end.
const artworkObserver = "IntersectionObserver" in window ? new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (entry.isIntersecting) {
      artworkObserver.unobserve(entry.target);
      entry.target.loadArtwork?.();
    }
  }
}, { rootMargin: "240px" }) : null;

function forgetPosters(container) {
  for (const poster of container.querySelectorAll(".poster")) artworkObserver?.unobserve(poster);
  container.replaceChildren();
}

function makePoster(item, eager = false) {
  const poster = element("div", "poster");
  poster.dataset.tone = Math.abs(Number(item.id) || 0) % 5;
  poster.setAttribute("aria-hidden", "true");
  const fallback = element("div", "poster-fallback");
  fallback.append(icon(mediaIcon(item.kind)), element("span", "fallback-name", item.title || "Untitled"));
  poster.append(fallback);
  const cache = state.artwork;
  poster.loadArtwork = async () => {
    const url = await cache?.load(item);
    if (!url || cache !== state.artwork || !poster.isConnected) return;
    const img = element("img");
    img.alt = "";
    img.decoding = "async";
    img.addEventListener("load", () => { fallback.hidden = true; });
    img.addEventListener("error", () => { fallback.hidden = false; img.remove(); });
    img.src = url;
    poster.append(img);
  };
  // The node is appended by the caller before observation or eager loading.
  requestAnimationFrame(() => {
    if (!poster.isConnected || cache !== state.artwork) return;
    if (eager || !artworkObserver) poster.loadArtwork();
    else artworkObserver.observe(poster);
  });
  return poster;
}

function resetSession() {
  clearTimeout(searchTimer);
  const client = state.client;
  state.client = null;
  closePlayer(false);
  for (const controller of state.requests.values()) controller.abort();
  state.requests.clear();
  artworkObserver?.disconnect();
  state.artwork?.dispose();
  state.artwork = null;
  Object.assign(state, {
    user: null, libraries: [], librariesLoaded: false, items: [], total: 0, offset: 0,
    browseLoaded: false, browseError: null, loading: false, scanning: false, continueEntries: [],
    continueLoaded: false, continueError: null, detailId: null, currentItem: null, progress: null,
    browseMode: "items", seriesId: null, seriesDetail: null, seriesLoaded: false, seriesError: null
  });
  forgetPosters($("grid"));
  forgetPosters($("continue-row"));
  forgetPosters($("detail-poster-wrap"));
  forgetPosters($("series-poster-wrap"));
  $("detail-content").hidden = true;
  $("detail-view").hidden = true;
  $("series-content").hidden = true;
  $("series-view").hidden = true;
  $("library-view").hidden = false;
  $("continue-section").hidden = true;
  $("toast").hidden = true;
  $("refresh-btn").disabled = false;
  $("refresh-btn").removeAttribute("aria-busy");
  $("refresh-btn").querySelector("span").textContent = "Scan media";
  setMenu(false);
  document.title = "Flex";
  return client;
}

function showLogin(message = "") {
  $("startup").hidden = true;
  $("app-view").hidden = true;
  $("login-view").hidden = false;
  $("login-error").textContent = message;
  $("login-error").hidden = !message;
}

function connect(server, token, user) {
  const client = new ApiClient(server, token, { onUnauthorized: () => {
    if (state.client !== client) return;
    writeStorage(tokenKey(server), null);
    resetSession();
    showLogin("Your session expired. Sign in again to continue.");
    $("password-input").value = "";
    $("password-input").focus();
  } });
  state.client = client;
  state.artwork = new ArtworkCache(client);
  state.user = user;
  return client;
}

function showApp(user) {
  state.user = user;
  $("startup").hidden = true;
  $("login-view").hidden = true;
  $("app-view").hidden = false;
  $("user-label").textContent = user.username;
  $("user-avatar").textContent = user.username.slice(0, 2).toUpperCase();
  $("server-label").textContent = new URL(state.client.server).host;
  $("server-label").title = state.client.server;
  $("offline-notice").hidden = navigator.onLine;
  readRoute();
  setView(state.view);
  renderLibraries();
  loadLibraries();
  reloadItems();
  loadContinueWatching();
  if (state.detailId != null) loadDetail(state.detailId);
  else if (state.seriesId != null) loadSeries(state.seriesId);
}

async function login(event) {
  event.preventDefault();
  if (state.authBusy) return;
  let server;
  try { server = normalizeServer($("server-input").value, location.origin); } catch (error) {
    showLogin(error.message);
    $("connection-settings").open = true;
    $("server-input").focus();
    return;
  }
  const username = $("username-input").value.trim();
  const password = $("password-input").value;
  if (!username || !password) return;
  state.authBusy = true;
  $("login-submit").disabled = true;
  $("login-submit").textContent = "Signing in...";
  $("login-error").hidden = true;
  try {
    const data = await new ApiClient(server).request("/auth/login", { method: "POST", body: {
      username, password, device_name: $("device-input").value.trim() || "Web browser"
    } });
    if (!data?.token || typeof data.user?.username !== "string") throw new Error("The server returned an incomplete sign-in response.");
    resetSession();
    writeStorage("flex.server", server);
    const persisted = writeStorage(tokenKey(server), data.token);
    $("server-input").value = server;
    $("password-input").value = "";
    connect(server, data.token, data.user);
    showApp(data.user);
    if (!persisted) toast("Browser storage is unavailable. You'll need to sign in again after closing this page.");
  } catch (error) {
    showLogin(error.message);
  } finally {
    state.authBusy = false;
    $("login-submit").disabled = false;
    $("login-submit").textContent = "Sign in";
  }
}

async function logout() {
  const client = state.client;
  if (!client) return;
  // Finish saving and stopping with the old credentials before revoking them.
  closePlayer(false);
  const cleanups = [...state.cleanups].filter(cleanup => cleanup.client === client).map(cleanup => cleanup.promise);
  writeStorage(tokenKey(client.server), null);
  resetSession();
  history.replaceState(null, "", location.pathname + location.search);
  showLogin();
  $("username-input").focus();
  await Promise.allSettled(cleanups);
  try { await client.request("/auth/logout", { method: "POST", keepalive: true }); } catch {
    if (!state.client) toast("Signed out of this browser. The server couldn't be reached to revoke the session.");
  }
}

function routeHash() {
  const params = new URLSearchParams();
  if (state.libraryId != null) params.set("library", state.libraryId);
  if (state.query) params.set("q", state.query);
  if (state.sort !== "title") params.set("sort", state.sort);
  if (state.seriesId != null) params.set("series", state.seriesId);
  if (state.detailId != null) params.set("item", state.detailId);
  return params.size ? "#" + params : location.pathname + location.search;
}

function writeRoute(replace = false) {
  history[replace ? "replaceState" : "pushState"]({
    flexItem: state.detailId,
    flexSeries: state.seriesId
  }, "", routeHash());
  lastHistoryURL = location.href;
}

function readRoute() {
  const params = new URLSearchParams(location.hash.slice(1));
  const id = value => /^\d+$/.test(value || "") ? Number(value) : null;
  state.libraryId = id(params.get("library"));
  state.query = params.get("q") || "";
  state.sort = SORTS.includes(params.get("sort")) ? params.get("sort") : "title";
  state.detailId = id(params.get("item"));
  state.seriesId = id(params.get("series"));
  $("search-input").value = state.query;
  $("search-clear").hidden = !state.query;
  $("sort-select").value = state.sort;
}

function browseKey() { return JSON.stringify([state.libraryId, state.query, state.sort]); }

function onHistory() {
  if (!state.user || lastHistoryURL === location.href) return;
  lastHistoryURL = location.href;
  if (location.hash === "#main-content") return;
  clearTimeout(searchTimer);
  const previous = browseKey();
  const previousDetail = state.detailId;
  const previousSeries = state.seriesId;
  readRoute();
  renderLibraries();
  renderHeading();
  renderContinue();
  if (previous !== browseKey()) reloadItems();
  if (state.detailId != null) loadDetail(state.detailId);
  else if (state.seriesId != null) {
    if (previousSeries !== state.seriesId) loadSeries(state.seriesId);
    else showSeriesView();
  } else if (previousDetail != null || previousSeries != null) showBrowse();
}

function selectLibrary(id) {
  clearTimeout(searchTimer);
  state.libraryId = id;
  state.query = "";
  $("search-input").value = "";
  $("search-clear").hidden = true;
  showBrowse(false);
  writeRoute();
  renderLibraries();
  renderHeading();
  renderContinue();
  reloadItems();
  setMenu(false);
  $("main-content").focus({ preventScroll: true });
  window.scrollTo(0, 0);
}

function setMenu(open) {
  document.querySelector(".sidebar").classList.toggle("is-expanded", open);
  $("menu-toggle").setAttribute("aria-expanded", String(open));
}

async function loadLibraries() {
  const client = state.client;
  if (!client) return;
  const controller = beginRequest("libraries");
  $("libraries-error").hidden = true;
  $("libraries-retry").disabled = true;
  try {
    const libraries = await client.request("/libraries", { signal: controller.signal });
    if (!current(client, controller)) return;
    if (!Array.isArray(libraries) || libraries.some(lib => !lib || !Number.isSafeInteger(lib.id) || typeof lib.name !== "string")) throw new Error("The server returned an unreadable library list.");
    state.libraries = libraries;
    state.librariesLoaded = true;
    renderLibraries();
    renderHeading();
    // A deep link into a TV library is fetched before libraries load, so the
    // browse mode can change from items to series once the kind is known.
    if (state.detailId == null && state.seriesId == null && browseModeOf() !== state.browseMode) reloadItems();
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    $("libraries-error-text").textContent = "Couldn't load libraries. " + error.message;
    $("libraries-error").hidden = false;
  } finally {
    if (current(client, controller)) $("libraries-retry").disabled = false;
  }
}

function renderLibraries() {
  const nav = $("libraries");
  const fragment = document.createDocumentFragment();
  const total = state.librariesLoaded ? state.libraries.reduce((sum, lib) => sum + (lib.item_count || 0), 0) : null;
  for (const lib of [{ id: null, name: "All media", kind: "all", item_count: total }, ...state.libraries]) {
    const node = button("lib-button", "", () => selectLibrary(lib.id));
    if (lib.id === state.libraryId) node.setAttribute("aria-current", "page");
    node.append(icon(lib.kind === "all" ? "library" : mediaIcon(lib.kind)), element("span", "lib-name", lib.name));
    if (lib.item_count != null) node.append(element("span", "lib-count", lib.item_count.toLocaleString()));
    fragment.append(node);
  }
  nav.replaceChildren(fragment);
}

function renderHeading() {
  const lib = activeLibrary();
  $("browse-heading").textContent = state.libraryId == null ? "All media" : lib?.name || "Library";
  const isSeries = browseModeOf() === "series";
  $("library-description").textContent = state.libraryId == null ? "Movies, episodes, and videos on your server." : `${lib ? lib.name + " on" : "Media from"} your server.`;
  $("view-label").textContent = state.query ? "Search results" : isSeries ? "All shows" : "All titles";
  document.title = (state.detailId != null || state.seriesId != null) ? document.title : $("browse-heading").textContent + " | Flex";
}

function setView(view) {
  state.view = view === "list" ? "list" : "grid";
  writeStorage("flex.view", state.view);
  $("grid").classList.toggle("is-list", state.view === "list");
  $("grid-view-btn").setAttribute("aria-pressed", String(state.view === "grid"));
  $("list-view-btn").setAttribute("aria-pressed", String(state.view === "list"));
}

function renderSkeletons() {
  forgetPosters($("grid"));
  for (let i = 0; i < 8; i++) {
    const node = element("div", "skeleton");
    node.setAttribute("aria-hidden", "true");
    const lines = element("div", "skeleton-lines");
    lines.append(element("span", "skeleton-line"), element("span", "skeleton-line short"));
    node.append(element("div", "poster"), lines);
    $("grid").append(node);
  }
}

function mediaCard(item) {
  const card = button("media-card", "", () => openDetail(item.id));
  card.dataset.itemId = item.id;
  card.dataset.browseId = item.id;
  card.setAttribute("aria-label", `${item.title || "Untitled"}${item.year ? ", " + item.year : ""}. View details`);
  const poster = makePoster(item);
  const hover = element("span", "poster-hover");
  hover.append(icon("info"));
  poster.append(hover);
  const body = element("span", "card-body");
  const meta = [item.year, formatRuntime(item.duration_ms)].filter(Boolean).join(" · ");
  body.append(element("span", "card-title", item.title || "Untitled"), element("span", "card-meta", meta || kindLabel(item.kind)));
  card.append(poster, body, element("span", "card-kind", kindLabel(item.kind)));
  return card;
}

function seriesCard(series) {
  const card = button("media-card", "", () => openSeries(series.id));
  card.dataset.seriesId = series.id;
  card.dataset.browseId = series.id;
  const { seasons, episodes } = seriesCounts(series);
  card.setAttribute("aria-label", `${series.title || "Untitled"}${series.year ? ", " + series.year : ""}. ${seasons}, ${episodes}. View series`);
  const poster = makePoster({ ...series, kind: "tv" });
  const hover = element("span", "poster-hover");
  hover.append(icon("info"));
  poster.append(hover);
  const body = element("span", "card-body");
  const meta = [series.year, seasons].filter(Boolean).join(" · ");
  body.append(element("span", "card-title", series.title || "Untitled"), element("span", "card-meta", meta || episodes));
  card.append(poster, body, element("span", "card-kind", episodes));
  return card;
}

function renderBrowseStatus() {
  $("grid").setAttribute("aria-busy", String(state.loading));
  const isSeries = state.browseMode === "series";
  const noun = isSeries ? "shows" : "titles";
  $("results-count").textContent = state.loading && state.browseAction === "refresh" ? `Refreshing ${noun}...` : state.loading && !state.browseLoaded ? `Loading ${noun}...` : state.browseLoaded ? countLabel(state.total) + (state.query ? ` matching "${state.query}"` : "") : "";
  $("loaded-count").textContent = state.browseLoaded && state.total > 0 ? `${state.items.length.toLocaleString()} of ${countLabel(state.total)} shown` : "";
  $("load-more").hidden = !state.browseLoaded || state.offset >= state.total;
  $("load-more").disabled = state.loading;
  $("load-more").textContent = state.loading ? "Loading..." : "Load more";
  $("browse-error").hidden = !state.browseError;
  $("browse-error-title").textContent = state.browseAction === "more" ? `Couldn't load more ${noun}` : state.browseAction === "refresh" ? "Couldn't refresh this library" : "Couldn't load your media";
  $("browse-error-text").textContent = state.browseError ? state.browseError.message + (state.browseAction === "refresh" ? " Previously loaded titles are shown below." : "") : "";
  const empty = state.browseLoaded && !state.loading && !state.browseError && !state.items.length;
  $("browse-empty").hidden = !empty;
  $("empty-title").textContent = state.query ? `No matching ${isSeries ? "shows" : "titles"}` : isSeries ? "No shows yet" : "No media yet";
  $("empty-description").textContent = state.query ? "Try a different title or clear your search to browse this library." : isSeries ? "Scan media to index episodes and group them into shows." : "Scan media to index files in your configured libraries.";
  $("empty-clear").hidden = !state.query;
}

async function reloadItems(preserve = false) {
  const client = state.client;
  if (!client) return;
  state.browseMode = browseModeOf();
  const controller = beginRequest("browse");
  preserve = preserve === true && state.browseLoaded;
  if (!preserve) Object.assign(state, { items: [], offset: 0, total: 0, browseLoaded: false });
  Object.assign(state, { browseError: null, browseAction: preserve ? "refresh" : "load", loading: true });
  renderHeading();
  if (!preserve) renderSkeletons();
  renderBrowseStatus();
  await fetchItems(client, controller, false);
}

async function loadMore() {
  if (state.loading || !state.client || state.offset >= state.total) return;
  const controller = beginRequest("browse");
  state.loading = true;
  state.browseAction = "more";
  state.browseError = null;
  renderBrowseStatus();
  await fetchItems(state.client, controller, true);
}

async function fetchItems(client, controller, append) {
  const offset = append ? state.offset : 0;
  const params = new URLSearchParams({ limit: state.limit, offset, sort: state.sort });
  if (state.libraryId != null) params.set("library_id", state.libraryId);
  if (state.query) params.set("q", state.query);
  const mode = state.browseMode || browseModeOf();
  const endpoint = mode === "series" ? "/series?" : "/items?";
  const validator = mode === "series" ? validSeries : validItem;
  const cardFn = mode === "series" ? seriesCard : mediaCard;
  try {
    const page = await client.request(endpoint + params, { signal: controller.signal });
    if (!current(client, controller)) return;
    if (!Array.isArray(page?.items) || page.items.some(item => !validator(item)) || !Number.isFinite(page.total) || page.total < 0) throw new Error("The server returned an unreadable media list.");
    if (!append) {
      forgetPosters($("grid"));
      state.items = [];
    }
    const seen = new Set(state.items.map(item => item.id));
    const additions = page.items.filter(item => !seen.has(item.id));
    state.items.push(...additions);
    // Offset counts server rows, even if a changing library returns duplicates.
    state.offset = offset + page.items.length;
    state.total = page.total;
    if (!page.items.length) state.offset = Math.max(state.offset, state.total);
    state.browseLoaded = true;
    const fragment = document.createDocumentFragment();
    additions.forEach(item => fragment.append(cardFn(item)));
    $("grid").append(fragment);
    if (append && state.offset >= state.total && document.activeElement === $("load-more")) {
      const firstAdded = additions.length ? $("grid").querySelector(`[data-browse-id="${additions[0].id}"]`) : $("grid").lastElementChild;
      (firstAdded || $("main-content")).focus();
    }
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    state.browseError = error;
    if (!append && !state.browseLoaded) forgetPosters($("grid"));
  } finally {
    if (current(client, controller)) {
      state.loading = false;
      renderBrowseStatus();
    }
  }
}

async function loadContinueWatching() {
  const client = state.client;
  if (!client) return;
  const controller = beginRequest("continue");
  $("continue-retry").disabled = true;
  try {
    const page = await client.request("/continue-watching?limit=20", { signal: controller.signal });
    if (!current(client, controller)) return;
    if (!Array.isArray(page?.items) || page.items.some(entry => !validItem(entry?.item) || !Number.isFinite(entry.state?.position_ms))) throw new Error("The server returned an unreadable watch list.");
    state.continueEntries = page.items;
    state.continueLoaded = true;
    state.continueError = null;
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    state.continueError = error;
  } finally {
    if (current(client, controller)) {
      $("continue-retry").disabled = false;
      renderContinue();
    }
  }
}

function renderContinue() {
  const entries = state.continueEntries.filter(({ item }) => state.libraryId == null || item.library_id === state.libraryId);
  $("continue-section").hidden = !!state.query || (!entries.length && !state.continueError);
  $("continue-error").hidden = !state.continueError;
  $("continue-error-text").textContent = state.continueError ? "Couldn't load your watch list. " + state.continueError.message : "";
  $("continue-count").textContent = entries.length ? titleCount(entries.length) : "";
  const row = $("continue-row");
  forgetPosters(row);
  for (const { item, state: watch } of entries) {
    const card = button("continue-card", "", () => startPlayback(item, watch.position_ms || 0));
    card.setAttribute("aria-label", `Resume ${item.title} from ${formatTime(watch.position_ms)}`);
    const progress = element("div", "watch-progress");
    progress.setAttribute("aria-hidden", "true");
    const bar = element("span");
    const duration = watch.duration_ms || item.duration_ms;
    bar.style.width = (duration ? Math.max(0, Math.min(100, watch.position_ms / duration * 100)) : 0) + "%";
    progress.append(bar);
    const body = element("span", "continue-body");
    const remaining = duration > watch.position_ms ? `${formatRuntime(duration - watch.position_ms)} left` : formatTime(watch.position_ms);
    body.append(element("span", "continue-title", item.title), element("span", "continue-meta", [item.year, remaining].filter(Boolean).join(" · ")));
    const play = element("span", "continue-play");
    play.setAttribute("aria-hidden", "true");
    play.append(icon("play"));
    card.append(makePoster(item), play, progress, body);
    row.append(card);
  }
}

function openSeries(id) {
  if (state.detailId == null && state.seriesId == null) {
    browseScroll = window.scrollY;
    browseFocus = id;
  }
  state.seriesId = id;
  writeRoute();
  loadSeries(id);
}

function openDetail(id) {
  if (state.detailId == null && state.seriesId == null) {
    browseScroll = window.scrollY;
    browseFocus = id;
  }
  state.detailId = id;
  writeRoute();
  loadDetail(id);
}

function showBrowse(restore = true) {
  for (const key of ["detail", "progress", "subtitles", "series"]) state.requests.get(key)?.abort();
  state.detailId = null;
  state.currentItem = null;
  state.progress = null;
  state.seriesId = null;
  state.seriesDetail = null;
  $("detail-view").hidden = true;
  $("series-view").hidden = true;
  $("library-view").hidden = false;
  renderHeading();
  if (restore) requestAnimationFrame(() => {
    window.scrollTo(0, browseScroll);
    const focus = browseFocus;
    const selector = focus != null ? `[data-browse-id="${Number(focus)}"]` : null;
    const card = selector ? $("grid").querySelector(selector) : null;
    (card || $("main-content")).focus({ preventScroll: true });
  });
}

function backFromDetail() {
  if (history.state?.flexItem === state.detailId) { history.back(); return; }
  if (state.seriesId != null) {
    state.detailId = null;
    writeRoute(true);
    showSeriesView();
  } else {
    showBrowse();
    writeRoute(true);
  }
}
function seriesBack() {
  if (history.state?.flexSeries === state.seriesId && history.state?.flexItem == null) { history.back(); return; }
  state.seriesId = null;
  showBrowse();
  writeRoute(true);
}
function showSeriesView() {
  $("library-view").hidden = true;
  $("detail-view").hidden = true;
  $("series-view").hidden = false;
}

async function loadSeries(id) {
  const client = state.client;
  if (!client) return;
  const controller = beginRequest("series");
  state.seriesId = id;
  state.seriesDetail = null;
  state.seriesLoaded = false;
  state.seriesError = null;
  showSeriesView();
  $("series-loading").hidden = false;
  $("series-error").hidden = true;
  $("series-content").hidden = true;
  $("series-title").textContent = "Series";
  window.scrollTo(0, 0);
  try {
    const detail = await client.request("/series/" + id, { signal: controller.signal });
    if (!current(client, controller) || state.seriesId !== id) return;
    if (!validSeries(detail?.series) || !Array.isArray(detail.seasons)) throw new Error("The server returned incomplete series information.");
    state.seriesDetail = detail;
    state.seriesLoaded = true;
    renderSeries(detail);
    $("series-loading").hidden = true;
    $("series-content").hidden = false;
    $("series-title").focus({ preventScroll: true });
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    $("series-loading").hidden = true;
    $("series-error-text").textContent = error.message;
    $("series-error").hidden = false;
    $("series-retry").focus({ preventScroll: true });
  }
}

function renderSeries(detail) {
  const series = detail.series;
  document.title = series.title + " | Flex";
  $("series-title").textContent = series.title;
  const counts = seriesCounts({ season_count: series.season_count || detail.seasons.length, episode_count: series.episode_count });
  $("series-meta").textContent = [series.year, counts.seasons, counts.episodes].filter(Boolean).join(" · ");
  $("series-overview").textContent = series.overview || "No description available for this series.";
  forgetPosters($("series-poster-wrap"));
  $("series-poster-wrap").append(makePoster({ ...series, kind: "tv" }, true));
  const container = $("series-seasons");
  container.replaceChildren();
  for (const season of detail.seasons) {
    const section = element("section", "season");
    section.append(element("h2", "season-heading", season.season_number > 0 ? `Season ${season.season_number}` : "Specials"));
    const list = element("ul", "episode-list");
    const episodesInOrder = [...season.episodes].sort((a, b) => (a.episode || 0) - (b.episode || 0));
    for (const episode of episodesInOrder) {
      const item = element("li");
      const entry = button("episode-button", "", () => openDetail(episode.id));
      entry.append(
        element("span", "episode-number", formatEpisodeNumber(episode.episode)),
        element("span", "episode-title", episode.title || "Untitled"),
        element("span", "episode-meta", formatRuntime(episode.duration_ms))
      );
      item.append(entry);
      list.append(item);
    }
    section.append(list);
    container.append(section);
  }
}

async function loadDetail(id) {
  const client = state.client;
  if (!client) return;
  const controller = beginRequest("detail");
  state.requests.get("subtitles")?.abort();
  state.detailId = id;
  state.currentItem = null;
  state.progress = null;
  $("library-view").hidden = true;
  $("series-view").hidden = true;
  $("detail-view").hidden = false;
  $("detail-back-label").textContent = state.seriesId != null ? "Back to series" : "Back to library";
  $("detail-loading").hidden = false;
  $("detail-error").hidden = true;
  $("detail-content").hidden = true;
  $("detail-progress-error").hidden = true;
  $("detail-resume").hidden = true;
  $("detail-play").disabled = true;
  $("detail-title").textContent = "Media details";
  $("sub-search-results").replaceChildren();
  $("sub-search-status").textContent = "";
  $("sub-search-btn").disabled = false;
  window.scrollTo(0, 0);
  loadProgress(id);
  try {
    const item = await client.request("/items/" + id, { signal: controller.signal });
    if (!current(client, controller) || state.detailId !== id) return;
    if (!item || item.id !== id || typeof item.title !== "string") throw new Error("The server returned incomplete media information.");
    state.currentItem = item;
    renderDetail(item);
    $("detail-loading").hidden = true;
    $("detail-content").hidden = false;
    $("detail-title").focus({ preventScroll: true });
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    $("detail-loading").hidden = true;
    $("detail-error-text").textContent = error.message;
    $("detail-error").hidden = false;
    $("detail-retry").focus({ preventScroll: true });
  }
}

async function loadProgress(id) {
  const client = state.client;
  if (!client) return;
  const controller = beginRequest("progress");
  $("progress-retry").disabled = true;
  $("detail-progress-error").hidden = true;
  try {
    const progress = await client.request("/progress/" + id, { signal: controller.signal });
    if (!current(client, controller) || state.detailId !== id) return;
    if (!progress || !Number.isFinite(progress.position_ms) || progress.position_ms < 0) throw new Error("The server returned an unreadable saved position.");
    state.progress = progress;
    renderResume();
  } catch (error) {
    if (!current(client, controller) || state.detailId !== id || ignoreError(error)) return;
    state.progress = null;
    if (error.status !== 404) $("detail-progress-error").hidden = false;
    renderResume();
  } finally {
    if (current(client, controller)) $("progress-retry").disabled = false;
  }
}

function renderResume() {
  const progress = state.progress;
  const resume = !!(progress?.position_ms > 0 && !progress.watched);
  $("detail-resume").hidden = !resume;
  $("detail-resume").textContent = resume ? "Resume from " + formatTime(progress.position_ms) : "Resume";
  $("detail-play").querySelector("span").textContent = resume ? "Play from beginning" : "Play";
  $("detail-play").classList.toggle("secondary", resume);
  $("detail-play").classList.toggle("primary", !resume);
}

function renderDetail(item) {
  document.title = item.title + " | Flex";
  $("detail-title").textContent = item.title;
  $("detail-kind").textContent = kindLabel(item.kind);
  $("detail-meta").textContent = [item.year, formatRuntime(item.duration_ms), item.video?.height ? item.video.height + "p" : null].filter(Boolean).join(" · ");
  $("detail-overview").textContent = item.overview || "No description available for this title.";
  forgetPosters($("detail-poster-wrap"));
  $("detail-poster-wrap").append(makePoster(item, true));
  const facts = $("detail-facts");
  facts.replaceChildren();
  const library = state.libraries.find(lib => lib.id === item.library_id);
  const fileSize = item.file_size > 0 ? (item.file_size / 1073741824 >= 1 ? (item.file_size / 1073741824).toFixed(1) + " GB" : Math.max(1, Math.round(item.file_size / 1048576)) + " MB") : null;
  for (const [label, value] of [
    ["Library", library?.name], ["Format", item.container?.split(",")[0]?.toUpperCase()],
    ["Video", item.video ? [item.video.codec?.toUpperCase(), item.video.width && item.video.height ? `${item.video.width} × ${item.video.height}` : null].filter(Boolean).join(" · ") : null],
    ["File size", fileSize]
  ]) {
    if (!value) continue;
    const fact = element("div");
    fact.append(element("dt", "", label), element("dd", "", value));
    facts.append(fact);
  }
  renderTracks($("detail-audio"), item.audio || [], "audio");
  renderTracks($("detail-subs"), item.subtitles || [], "subtitles");
  $("detail-play").disabled = false;
  renderResume();
}

function renderTracks(list, tracks, type) {
  list.replaceChildren();
  for (const track of tracks) {
    const row = element("li");
    const label = track.title || track.language || "Track " + track.index;
    const meta = [track.codec?.toUpperCase(), track.channels ? `${track.channels} channels` : null, track.forced ? "Forced" : null, track.default ? "Default" : null].filter(Boolean).join(" · ");
    row.append(element("span", "", label), element("small", "", meta));
    list.append(row);
  }
  if (!tracks.length) list.append(element("li", "muted", type === "audio" ? "No audio tracks found." : "No subtitles attached."));
}

async function searchSubtitles() {
  const item = state.currentItem;
  const client = state.client;
  if (!item || !client) return;
  const controller = beginRequest("subtitles");
  $("sub-search-btn").disabled = true;
  $("sub-search-status").textContent = "Searching for subtitles...";
  $("sub-search-results").replaceChildren();
  try {
    const results = await client.request("/subtitles/search?item_id=" + item.id, { signal: controller.signal });
    if (!current(client, controller) || state.detailId !== item.id) return;
    if (!Array.isArray(results)) throw new Error("The server returned an unreadable subtitle list.");
    $("sub-search-status").textContent = results.length ? `${results.length} results. Choose a subtitle to download.` : "No subtitles found. Online providers must be configured on your server.";
    for (const result of results) {
      const row = element("li");
      const label = element("span", "", [result.language, result.title, result.release].filter(Boolean).join(" · "));
      const download = button("secondary", "Download", async () => {
        download.disabled = true;
        download.textContent = "Downloading...";
        try {
          await client.request("/subtitles/download", { method: "POST", body: {
            provider: result.provider, id: result.id, language: result.language, item_id: item.id
          } });
          if (state.client !== client) return;
          toast("Subtitle downloaded for " + item.title + ".");
          if (state.detailId === item.id) loadDetail(item.id);
        } catch (error) {
          if (!current(client, controller) || ignoreError(error)) return;
          download.disabled = false;
          download.textContent = "Try download again";
          $("sub-search-status").textContent = "Couldn't download the subtitle. " + error.message;
        }
      });
      row.append(label, download);
      $("sub-search-results").append(row);
    }
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    $("sub-search-status").textContent = "Couldn't search for subtitles. " + error.message;
  } finally {
    if (current(client, controller)) $("sub-search-btn").disabled = false;
  }
}

async function startPlayback(item, startMs = 0) {
  const client = state.client;
  if (!client || !item) return;
  const previousCleanup = closePlayer(false);
  const playback = {
    client, item, startMs: Math.max(0, Number(startMs) || 0), attempt: ++state.playbackAttempt,
    session: null, hls: null, ready: false, failed: false, offsetMs: 0,
    durationMs: item.duration_ms, progressQueue: Promise.resolve(), timer: null, metadata: null,
    networkRecoveries: 0, mediaRecoveries: 0
  };
  state.player = playback;
  $("player-title").textContent = item.title;
  $("player-status").textContent = "Starting playback...";
  $("progress-status").textContent = "";
  $("player-retry").hidden = true;
  $("player-dialog").showModal();
  $("player-close").focus();
  try {
    await previousCleanup;
    if (state.player !== playback || state.client !== client) return;
    playback.startRequest = client.request("/playback/start", { method: "POST", body: {
      item_id: item.id, capabilities: browserCapabilities($("player-video"), window.Hls),
      // Zero is meaningful: omitting it lets the server resume saved progress.
      start_position_ms: playback.startMs
    } });
    const session = await playback.startRequest;
    if (state.player !== playback || state.playbackAttempt !== playback.attempt || state.client !== client) {
      if (session?.session_id) stopPlaybackSession(playback, session);
      return;
    }
    playback.session = session;
    if (!session?.session_id || !session.url || !["file", "hls"].includes(session.stream_kind)) throw new Error("The server returned an incomplete playback session.");
    playback.durationMs = item.duration_ms || session.duration_ms;
    // HLS begins at the requested position on the server. Direct files need a
    // client seek and already report their time relative to the original file.
    playback.offsetMs = session.stream_kind === "hls" ? playback.startMs : 0;
    openPlayer(playback);
  } catch (error) {
    if (state.player !== playback || ignoreError(error)) return;
    playbackError(playback, "Couldn't start playback. " + error.message);
  }
}

function openPlayer(playback) {
  const video = $("player-video");
  const session = playback.session;
  const url = playback.client.url(session.url);
  playback.metadata = () => {
    if (state.player !== playback) return;
    if (session.stream_kind === "file" && playback.startMs > 0) {
      const target = playback.startMs / 1000;
      try { video.currentTime = Number.isFinite(video.duration) ? Math.min(target, Math.max(0, video.duration - .1)) : target; } catch {
        playbackError(playback, "Couldn't seek to your saved position. Try playing from the beginning.");
        return;
      }
    }
    playback.ready = true;
    $("player-status").textContent = "";
  };
  video.addEventListener("loadedmetadata", playback.metadata, { once: true });
  // canPlayType("maybe") is only a hint. Prefer hls.js when MediaSource is
  // available, then fall back to native HLS on browsers without MSE support.
  if (session.stream_kind === "hls" && window.Hls?.isSupported()) {
    const hls = new window.Hls({ enableWorker: true, startPosition: 0 });
    playback.hls = hls;
    hls.on(window.Hls.Events.ERROR, (_event, data) => {
      if (state.player !== playback || !data.fatal) return;
      if (data.type === window.Hls.ErrorTypes.NETWORK_ERROR && playback.networkRecoveries++ < 2) {
        $("player-status").textContent = "Reconnecting to the stream...";
        hls.startLoad();
      } else if (data.type === window.Hls.ErrorTypes.MEDIA_ERROR && playback.mediaRecoveries++ < 1) {
        hls.recoverMediaError();
      } else playbackError(playback, "The stream couldn't be played. Check your connection, then try again.");
    });
    hls.loadSource(url);
    hls.attachMedia(video);
  } else if (session.stream_kind === "hls" && !video.canPlayType("application/vnd.apple.mpegurl")) {
    throw new Error("This browser cannot play HLS streams. Try a browser with HLS support.");
  } else video.src = url;
  video.play().catch(error => {
    if (state.player !== playback || error.name === "AbortError") return;
    if (error.name === "NotAllowedError") $("player-status").textContent = "Ready to play. Press Play in the video controls.";
    else playbackError(playback, "This browser couldn't play the stream. Try again or check the media file on your server.");
  });
  playback.timer = setInterval(() => saveProgress(false), 10000);
}

function playbackError(playback, message) {
  if (state.player !== playback || playback.failed) return;
  playback.failed = true;
  clearInterval(playback.timer);
  saveProgress(false);
  $("player-video").pause();
  playback.hls?.destroy();
  playback.hls = null;
  if (playback.session) stopPlaybackSession(playback, playback.session);
  $("player-status").textContent = message;
  $("player-retry").hidden = false;
}

function stopSession(client, session) {
  return client.request("/playback/stop", { method: "POST", body: { session_id: session.session_id }, keepalive: true }).catch(() => {});
}

function stopPlaybackSession(playback, session) {
  if (!session?.session_id) return Promise.resolve();
  playback.stopPromise ??= stopSession(playback.client, session);
  return playback.stopPromise;
}

function saveProgress(keepalive = false, ended = false, playback = state.player) {
  if (!playback) return Promise.resolve();
  const video = $("player-video");
  const payload = progressPayload(playback, video.currentTime, video.duration, ended);
  if (!payload) return playback.progressQueue;
  // Keep one write in flight and only the newest pending position. A slow
  // connection cannot accumulate old positions or write them out of order.
  playback.nextProgress = { payload, keepalive };
  if (!playback.writingProgress) {
    playback.writingProgress = true;
    playback.progressQueue = (async () => {
      while (playback.nextProgress) {
        const update = playback.nextProgress;
        playback.nextProgress = null;
        try {
          await playback.client.request("/progress/" + playback.item.id, { method: "PUT", body: update.payload, keepalive: update.keepalive });
          if (state.player === playback) $("progress-status").textContent = "";
        } catch (error) {
          if (state.player === playback && !ignoreError(error)) $("progress-status").textContent = "Your position couldn't be saved. Flex will try again.";
        }
      }
      playback.writingProgress = false;
    })();
  }
  return playback.progressQueue;
}

function closePlayer(refresh = true) {
  const playback = state.player;
  ++state.playbackAttempt;
  if (!playback) return Promise.resolve();
  const saved = saveProgress(true, $("player-video").ended, playback);
  state.player = null;
  clearInterval(playback.timer);
  const video = $("player-video");
  if (playback.metadata) video.removeEventListener("loadedmetadata", playback.metadata);
  video.pause();
  playback.hls?.destroy();
  playback.hls = null;
  video.removeAttribute("src");
  video.load();
  $("player-dialog").close();
  const stopped = playback.session ? stopPlaybackSession(playback, playback.session)
    : playback.startRequest ? playback.startRequest.then(session => stopPlaybackSession(playback, session)).catch(() => {})
      : Promise.resolve();
  const cleanup = Promise.allSettled([saved, stopped]);
  const entry = { client: playback.client, promise: cleanup };
  state.cleanups.add(entry);
  cleanup.then(() => {
    state.cleanups.delete(entry);
    if (!refresh || state.client !== playback.client) return;
    loadContinueWatching();
    if (state.detailId === playback.item.id) loadProgress(playback.item.id);
  });
  return cleanup;
}

function updateSearch() {
  state.query = $("search-input").value.trim();
  $("search-clear").hidden = !state.query;
  writeRoute(true);
  renderContinue();
  reloadItems();
}

function clearSearch() {
  clearTimeout(searchTimer);
  $("search-input").value = "";
  updateSearch();
  $("search-input").focus();
}

async function refreshView() {
  if (!state.client) return;
  await Promise.allSettled([loadLibraries(), reloadItems(true), loadContinueWatching()]);
}

function scanSummary(report) {
  if (!report || !["libraries", "scanned", "added", "updated", "removed"].every(key => Number.isSafeInteger(report[key])) || !Array.isArray(report.errors)) {
    throw new Error("The server returned an unreadable scan report.");
  }
  if (report.libraries === 0) return "No media libraries are configured.";
  let summary = `Scan complete. ${report.scanned.toLocaleString()} files checked, ${report.added.toLocaleString()} added, ${report.updated.toLocaleString()} updated, and ${report.removed.toLocaleString()} removed.`;
  if (report.errors.length) {
    const firstError = typeof report.errors[0] === "string" ? report.errors[0] : "A library could not be scanned.";
    summary += ` ${report.errors.length.toLocaleString()} scan ${report.errors.length === 1 ? "error" : "errors"}. ${firstError.slice(0, 180)}`;
    if (firstError.length > 180) summary += "…";
  }
  return summary;
}

async function scanMedia() {
  const client = state.client;
  if (!client || state.scanning) return;
  const controller = beginRequest("scan");
  state.scanning = true;
  const refreshButton = $("refresh-btn");
  const refreshLabel = refreshButton.querySelector("span");
  refreshButton.disabled = true;
  refreshButton.setAttribute("aria-busy", "true");
  refreshLabel.textContent = "Scanning…";
  toast("Scanning your media libraries…");
  try {
    const report = await client.request("/scan", { method: "POST", signal: controller.signal, timeoutMs: 0 });
    if (!current(client, controller)) return;
    await refreshView();
    if (!current(client, controller)) return;
    toast(scanSummary(report));
  } catch (error) {
    if (current(client, controller) && !ignoreError(error)) toast("Couldn't scan media. " + error.message);
  } finally {
    if (state.client === client) {
      state.scanning = false;
      refreshButton.disabled = false;
      refreshButton.removeAttribute("aria-busy");
      refreshLabel.textContent = "Scan media";
    }
  }
}

function bindEvents() {
  $("login-form").addEventListener("submit", login);
  document.querySelector(".skip-link").addEventListener("click", event => {
    event.preventDefault();
    $("main-content").focus();
    $("main-content").scrollIntoView();
  });
  $("logout-btn").addEventListener("click", logout);
  $("home-link").addEventListener("click", event => { event.preventDefault(); selectLibrary(null); });
  $("menu-toggle").addEventListener("click", () => setMenu($("menu-toggle").getAttribute("aria-expanded") !== "true"));
  $("refresh-btn").addEventListener("click", scanMedia);
  $("libraries-retry").addEventListener("click", loadLibraries);
  $("continue-retry").addEventListener("click", loadContinueWatching);
  $("browse-retry").addEventListener("click", () => state.browseAction === "more" ? loadMore() : reloadItems(state.browseAction === "refresh"));
  $("load-more").addEventListener("click", loadMore);
  $("grid-view-btn").addEventListener("click", () => setView("grid"));
  $("list-view-btn").addEventListener("click", () => setView("list"));
  $("search-input").addEventListener("input", () => {
    clearTimeout(searchTimer);
    // Cancel immediately, before the debounce, to prevent an older query flashing back.
    state.requests.get("browse")?.abort();
    $("search-clear").hidden = !$("search-input").value;
    searchTimer = setTimeout(updateSearch, 250);
  });
  $("search-input").addEventListener("keydown", event => {
    if (event.key === "Enter") { clearTimeout(searchTimer); updateSearch(); }
    if (event.key === "Escape") { event.preventDefault(); clearSearch(); }
  });
  $("search-clear").addEventListener("click", clearSearch);
  $("empty-clear").addEventListener("click", clearSearch);
  $("sort-select").addEventListener("change", () => {
    clearTimeout(searchTimer);
    state.sort = $("sort-select").value;
    state.query = $("search-input").value.trim();
    writeRoute();
    renderContinue();
    reloadItems();
  });
  $("detail-back").addEventListener("click", backFromDetail);
  $("detail-retry").addEventListener("click", () => loadDetail(state.detailId));
  $("series-back").addEventListener("click", seriesBack);
  $("series-retry").addEventListener("click", () => loadSeries(state.seriesId));
  $("progress-retry").addEventListener("click", () => loadProgress(state.detailId));
  $("detail-play").addEventListener("click", () => startPlayback(state.currentItem, 0));
  $("detail-resume").addEventListener("click", () => startPlayback(state.currentItem, state.progress?.position_ms || 0));
  $("sub-search-btn").addEventListener("click", searchSubtitles);
  $("player-close").addEventListener("click", () => closePlayer());
  $("player-dialog").addEventListener("cancel", event => { event.preventDefault(); closePlayer(); });
  $("player-dialog").addEventListener("close", () => {
    if (!$("player-dialog").open && state.player) closePlayer();
  });
  $("player-retry").addEventListener("click", () => {
    const playback = state.player;
    if (!playback) return;
    const payload = progressPayload(playback, $("player-video").currentTime, $("player-video").duration);
    startPlayback(playback.item, payload?.position_ms ?? playback.startMs);
  });
  const video = $("player-video");
  video.addEventListener("pause", () => saveProgress());
  video.addEventListener("ended", () => saveProgress(false, true));
  video.addEventListener("playing", () => { if (state.player && !state.player.failed) $("player-status").textContent = ""; });
  video.addEventListener("waiting", () => { if (state.player && !state.player.failed) $("player-status").textContent = "Buffering..."; });
  video.addEventListener("error", () => {
    if (state.player && !state.player.hls) playbackError(state.player, "The video couldn't be played. Check the media file and your connection, then try again.");
  });
  document.addEventListener("visibilitychange", () => { if (document.hidden) saveProgress(true); });
  window.addEventListener("pagehide", () => closePlayer(false));
  window.addEventListener("popstate", onHistory);
  window.addEventListener("hashchange", onHistory);
  window.addEventListener("offline", () => { $("offline-notice").hidden = false; });
  window.addEventListener("online", () => {
    $("offline-notice").hidden = true;
    if (state.user && (state.browseError || state.continueError || !state.librariesLoaded)) refreshView();
  });
}

async function init() {
  bindEvents();
  state.view = readStorage("flex.view", "grid");
  let server;
  try { server = normalizeServer(readStorage("flex.server", location.origin), location.origin); } catch {
    server = location.origin;
    writeStorage("flex.server", null);
  }
  $("server-input").value = server;
  const token = readStorage(tokenKey(server));
  if (!token) { showLogin(); return; }
  const client = connect(server, token, null);
  const controller = beginRequest("auth");
  try {
    const user = await client.request("/auth/me", { signal: controller.signal });
    if (!current(client, controller)) return;
    if (typeof user?.username !== "string") throw new Error("The server returned an incomplete account response.");
    showApp(user);
  } catch (error) {
    if (!current(client, controller) || ignoreError(error)) return;
    resetSession();
    showLogin(error.message);
  }
}

init().catch(error => {
  resetSession();
  showLogin("Couldn't open Flex. " + error.message);
});
