//! Asynchronous HTTP client for the Flex API.
//!
//! All network I/O runs on a Tokio runtime living on a dedicated background
//! thread. Results are handed back to the GTK main context via
//! [`glib::spawn_future_local`], so UI code never blocks on the network.

use std::fmt::Display;
use std::future::Future;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use flex_protocol::{
    artwork_path, continue_watching_path, health_path, item_path, items_path, libraries_path,
    login_path, logout_path, me_path, playback_start_path, playback_stop_path, progress_item_path,
    scan_path, series_artwork_path, series_item_path, series_path, ContinueWatchingEntry, Health,
    ItemQuery, Library, LoginRequest, LoginResponse, MediaItem, MediaKind, Page, PlaybackSession,
    PlaybackStartRequest, ScanReport, Series, SeriesDetail, SeriesQuery, StopPlaybackRequest,
    UpdateProgressRequest, User, WatchState,
};
use gtk4::glib;

/// Encode a single query-string component. `space` becomes `+`, everything
/// outside the unreserved set is percent-encoded. This mirrors what the server
/// (and most query parsers) expect for `application/x-www-form-urlencoded`.
fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0f));
            }
        }
    }
    out
}

/// Render a nibble (0..=15) as an uppercase hexadecimal ASCII character.
fn hex_digit(nibble: u8) -> char {
    char::from_digit(u32::from(nibble), 16)
        .unwrap_or('0')
        .to_ascii_uppercase()
}

fn media_kind_str(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Episode => "episode",
        MediaKind::Other => "other",
    }
}

fn push_param(out: &mut Vec<String>, key: &str, value: impl Display) {
    out.push(format!("{key}={}", encode_component(&value.to_string())));
}

/// Serialise an [`ItemQuery`] into a query string (without the leading `?`).
///
/// Parameters are emitted in a stable order so the output is easy to test and
/// deterministic on the wire.
pub fn build_query(query: &ItemQuery) -> String {
    let mut params = Vec::new();
    if let Some(library_id) = query.library_id {
        push_param(&mut params, "library_id", library_id);
    }
    if let Some(kind) = query.kind {
        push_param(&mut params, "kind", media_kind_str(kind));
    }
    if let Some(q) = query.q.as_deref().filter(|q| !q.is_empty()) {
        push_param(&mut params, "q", q);
    }
    if let Some(sort) = query.sort.as_deref().filter(|s| !s.is_empty()) {
        push_param(&mut params, "sort", sort);
    }
    if let Some(limit) = query.limit {
        push_param(&mut params, "limit", limit);
    }
    if let Some(offset) = query.offset {
        push_param(&mut params, "offset", offset);
    }
    params.join("&")
}

/// Serialise a [`SeriesQuery`] into a query string (without the leading `?`).
pub fn build_series_query(query: &SeriesQuery) -> String {
    let mut params = Vec::new();
    if let Some(library_id) = query.library_id {
        push_param(&mut params, "library_id", library_id);
    }
    if let Some(q) = query.q.as_deref().filter(|q| !q.is_empty()) {
        push_param(&mut params, "q", q);
    }
    if let Some(sort) = query.sort.as_deref().filter(|s| !s.is_empty()) {
        push_param(&mut params, "sort", sort);
    }
    if let Some(limit) = query.limit {
        push_param(&mut params, "limit", limit);
    }
    if let Some(offset) = query.offset {
        push_param(&mut params, "offset", offset);
    }
    params.join("&")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    Unauthorized,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ApiError {}

/// Join a server-relative path onto a base URL.
///
/// The base is assumed to have no trailing slash (it is trimmed in
/// [`ApiClient::new`]). Absolute URLs are returned unchanged, and the path's
/// own query string (`?t=…`) is preserved verbatim.
pub fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// Small convenience wrapper around [`reqwest::Client`] plus the Tokio runtime
/// handle used to drive requests.
#[derive(Clone)]
pub struct ApiClient {
    base: Arc<RwLock<String>>,
    http: reqwest::Client,
    handle: tokio::runtime::Handle,
    token: Arc<RwLock<Option<String>>>,
}

impl ApiClient {
    pub fn new(base: impl Into<String>, handle: tokio::runtime::Handle) -> Self {
        let base = base.into();
        let base = base.trim_end_matches('/').to_string();
        Self {
            base: Arc::new(RwLock::new(base)),
            http: reqwest::Client::new(),
            handle,
            token: Arc::new(RwLock::new(None)),
        }
    }

    /// Replace the configured server base URL (used by the login page).
    pub fn set_base(&self, base: impl Into<String>) {
        let base = base.into();
        *self.base.write().expect("base lock poisoned") = base.trim_end_matches('/').to_string();
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().expect("token lock poisoned") = token;
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().expect("token lock poisoned").clone()
    }

    pub fn clear_token(&self) {
        self.set_token(None);
    }

    /// Base server URL without a trailing slash.
    pub fn base_url(&self) -> String {
        self.base.read().expect("base lock poisoned").clone()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url(), path)
    }

    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let token = self.token.read().expect("token lock poisoned").clone();
        if let Some(token) = token {
            req.bearer_auth(token)
        } else {
            req
        }
    }

    /// Join a server-relative path (which may already carry a query string)
    /// onto the configured base URL. Used to turn a `PlaybackSession.url` into
    /// an absolute URL mpv can open.
    pub fn absolute_url(&self, path: &str) -> String {
        join_url(&self.base_url(), path)
    }

    /// Absolute URL for an item's artwork of the given type (`poster`/`backdrop`).
    pub fn artwork_url(&self, id: i64, kind: &str) -> String {
        format!("{}{}?type={}", self.base_url(), artwork_path(id), kind)
    }

    /// Absolute URL for a series' artwork of the given type (`poster`/`backdrop`).
    pub fn series_artwork_url(&self, id: i64, kind: &str) -> String {
        format!(
            "{}{}?type={}",
            self.base_url(),
            series_artwork_path(id),
            kind
        )
    }

    pub async fn health(&self) -> Result<Health> {
        let resp = self
            .http
            .get(self.url(&health_path()))
            .send()
            .await
            .context("request to /health failed")?;
        parse_json(resp, "/health").await
    }

    pub async fn libraries(&self) -> Result<Vec<Library>> {
        let resp = self
            .add_auth(self.http.get(self.url(&libraries_path())))
            .send()
            .await
            .context("request to /libraries failed")?;
        parse_json(resp, "/libraries").await
    }

    pub async fn items(&self, query: &ItemQuery) -> Result<Page<MediaItem>> {
        let qs = build_query(query);
        let path = items_path();
        let url = if qs.is_empty() {
            self.url(&path)
        } else {
            format!("{}{}?{}", self.base_url(), path, qs)
        };
        let resp = self
            .add_auth(self.http.get(url))
            .send()
            .await
            .context("request to /items failed")?;
        parse_json(resp, "/items").await
    }

    /// Fetch a filtered, sorted page of series using the current authentication token.
    pub async fn series(&self, query: &SeriesQuery) -> Result<Page<Series>> {
        let qs = build_series_query(query);
        let path = series_path();
        let url = if qs.is_empty() {
            self.url(&path)
        } else {
            format!("{}{}?{}", self.base_url(), path, qs)
        };
        let resp = self
            .add_auth(self.http.get(url))
            .send()
            .await
            .context("request to /series failed")?;
        parse_json(resp, "/series").await
    }

    /// Fetch series metadata and its season groups, propagating HTTP or decoding errors.
    pub async fn series_detail(&self, id: i64) -> Result<SeriesDetail> {
        let resp = self
            .add_auth(self.http.get(self.url(&series_item_path(id))))
            .send()
            .await
            .with_context(|| format!("request to /series/{id} failed"))?;
        parse_json(resp, "series").await
    }

    /// Fetch raw series artwork bytes, or `None` when there is none (404).
    pub async fn series_artwork_bytes(&self, id: i64, kind: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .add_auth(self.http.get(self.series_artwork_url(id, kind)))
            .send()
            .await
            .with_context(|| format!("request for artwork of series {id} failed"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp
            .error_for_status()
            .with_context(|| format!("artwork request for series {id} returned an error"))?;
        let bytes = resp.bytes().await.context("failed to read artwork bytes")?;
        Ok(Some(bytes.to_vec()))
    }

    pub async fn item(&self, id: i64) -> Result<MediaItem> {
        let resp = self
            .add_auth(self.http.get(self.url(&item_path(id))))
            .send()
            .await
            .with_context(|| format!("request to /items/{id} failed"))?;
        parse_json(resp, "item").await
    }

    pub async fn scan(&self) -> Result<ScanReport> {
        let resp = self
            .add_auth(self.http.post(self.url(&scan_path())))
            .send()
            .await
            .context("request to /scan failed")?;
        parse_json(resp, "/scan").await
    }

    /// Fetch raw artwork bytes. Returns `None` when the server replies `404`
    /// (no artwork available).
    pub async fn artwork_bytes(&self, id: i64, kind: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .add_auth(self.http.get(self.artwork_url(id, kind)))
            .send()
            .await
            .with_context(|| format!("request for artwork of item {id} failed"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp
            .error_for_status()
            .with_context(|| format!("artwork request for item {id} returned an error"))?;
        let bytes = resp.bytes().await.context("failed to read artwork bytes")?;
        Ok(Some(bytes.to_vec()))
    }

    /// Exchange credentials for a device token.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        device_name: Option<&str>,
    ) -> Result<LoginResponse> {
        let body = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            device_name: device_name.map(|s| s.to_string()),
        };
        let resp = self
            .http
            .post(self.url(&login_path()))
            .json(&body)
            .send()
            .await
            .context("request to /auth/login failed")?;
        parse_json(resp, "login").await
    }

    /// Revoke the current device token.
    pub async fn logout(&self) -> Result<()> {
        let resp = self
            .add_auth(self.http.post(self.url(&logout_path())))
            .send()
            .await
            .context("request to /auth/logout failed")?;
        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("server returned {status} for /auth/logout: {body}");
        }
        Ok(())
    }

    /// Fetch the authenticated user.
    pub async fn me(&self) -> Result<User> {
        let resp = self
            .add_auth(self.http.get(self.url(&me_path())))
            .send()
            .await
            .context("request to /auth/me failed")?;
        parse_json(resp, "/auth/me").await
    }

    /// Start a playback session.
    pub async fn playback_start(&self, request: &PlaybackStartRequest) -> Result<PlaybackSession> {
        let resp = self
            .add_auth(self.http.post(self.url(&playback_start_path())))
            .json(request)
            .send()
            .await
            .context("request to /playback/start failed")?;
        parse_json(resp, "playback/start").await
    }

    /// Stop a playback session.
    pub async fn playback_stop(&self, session_id: &str) -> Result<()> {
        let body = StopPlaybackRequest {
            session_id: session_id.to_string(),
        };
        let resp = self
            .add_auth(self.http.post(self.url(&playback_stop_path())))
            .json(&body)
            .send()
            .await
            .context("request to /playback/stop failed")?;
        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("server returned {status} for /playback/stop: {body}");
        }
        Ok(())
    }

    /// Save playback progress for an item.
    pub async fn update_progress(
        &self,
        item_id: i64,
        request: &UpdateProgressRequest,
    ) -> Result<WatchState> {
        let resp = self
            .add_auth(self.http.put(self.url(&progress_item_path(item_id))))
            .json(request)
            .send()
            .await
            .context("request to /progress failed")?;
        parse_json(resp, "progress").await
    }

    /// Fetch saved progress for a single item, or `None` when there is none.
    pub async fn progress_for_item(&self, item_id: i64) -> Result<Option<WatchState>> {
        let resp = self
            .add_auth(self.http.get(self.url(&progress_item_path(item_id))))
            .send()
            .await
            .context("request to /progress failed")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(parse_json(resp, "progress").await?))
    }

    /// List resumable ("continue watching") entries.
    pub async fn continue_watching(&self) -> Result<Vec<ContinueWatchingEntry>> {
        let resp = self
            .add_auth(self.http.get(self.url(&continue_watching_path())))
            .send()
            .await
            .context("request to /continue-watching failed")?;
        let page: Page<ContinueWatchingEntry> = parse_json(resp, "continue-watching").await?;
        Ok(page.items)
    }

    /// Spawn `future` on the Tokio runtime and invoke `on_done` on the GTK main
    /// context once it resolves. This is the sole bridge between networking and
    /// the UI thread.
    pub fn dispatch<F, T>(&self, future: F, on_done: impl FnOnce(Result<T>) + 'static)
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let join = self.handle.spawn(future);
        glib::spawn_future_local(async move {
            let result = match join.await {
                Ok(result) => result,
                Err(err) => Err(anyhow::anyhow!("background task failed: {err}")),
            };
            on_done(result);
        });
    }
}

#[allow(dead_code)]
fn check_response(resp: reqwest::Response, _what: &str) -> Result<reqwest::Response> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let api_err = ApiError {
            kind: ApiErrorKind::Unauthorized,
            message: "unauthorized".to_string(),
        };
        return Err(anyhow::anyhow!(api_err));
    }
    Ok(resp)
}

async fn parse_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    what: &str,
) -> Result<T> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let api_err = ApiError {
            kind: ApiErrorKind::Unauthorized,
            message: "unauthorized".to_string(),
        };
        anyhow::bail!(api_err);
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("server returned {status} for {what}: {body}");
    }
    resp.json::<T>()
        .await
        .with_context(|| format!("failed to decode JSON response for {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_has_no_params() {
        assert_eq!(build_query(&ItemQuery::default()), "");
    }

    #[test]
    fn full_query_is_encoded() {
        let query = ItemQuery {
            library_id: Some(3),
            kind: Some(MediaKind::Movie),
            q: Some("blade runner".into()),
            sort: Some("-year".into()),
            limit: Some(50),
            offset: Some(10),
        };
        assert_eq!(
            build_query(&query),
            "library_id=3&kind=movie&q=blade+runner&sort=-year&limit=50&offset=10"
        );
    }

    #[test]
    fn special_characters_are_percent_encoded() {
        let query = ItemQuery {
            q: Some("héllo/world?x=1".into()),
            ..ItemQuery::default()
        };
        assert_eq!(build_query(&query), "q=h%C3%A9llo%2Fworld%3Fx%3D1");
    }

    #[test]
    fn blank_search_is_omitted() {
        let query = ItemQuery {
            q: Some(String::new()),
            ..ItemQuery::default()
        };
        assert_eq!(build_query(&query), "");
    }

    /// Verify series query encoding includes browse filters and omits the item kind.
    #[test]
    fn series_query_has_no_kind_param() {
        let query = SeriesQuery {
            library_id: Some(4),
            q: Some("the office".into()),
            sort: Some("-year".into()),
            limit: Some(50),
            offset: Some(0),
        };
        assert_eq!(
            build_series_query(&query),
            "library_id=4&q=the+office&sort=-year&limit=50&offset=0"
        );
        assert_eq!(build_series_query(&SeriesQuery::default()), "");
    }

    #[test]
    fn joins_base_and_session_path() {
        assert_eq!(
            join_url(
                "http://127.0.0.1:8096",
                "/api/v1/playback/abc/master.m3u8?t=tok"
            ),
            "http://127.0.0.1:8096/api/v1/playback/abc/master.m3u8?t=tok"
        );
        // Trailing slashes on the base are tolerated.
        assert_eq!(join_url("http://host/", "/x"), "http://host/x");
        // Absolute URLs pass through untouched.
        assert_eq!(
            join_url("http://host", "https://cdn.example/x.m3u8"),
            "https://cdn.example/x.m3u8"
        );
    }

    #[test]
    fn session_url_absolute_join() {
        let handle = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .handle()
            .clone();
        let api = ApiClient::new("http://127.0.0.1:8096/", handle);
        assert_eq!(
            api.absolute_url("/api/v1/playback/s/master.m3u8?t=t"),
            "http://127.0.0.1:8096/api/v1/playback/s/master.m3u8?t=t"
        );
    }
}
