//! Asynchronous HTTP client for the Flex API.
//!
//! All network I/O runs on a Tokio runtime living on a dedicated background
//! thread. Results are handed back to the GTK main context via
//! [`glib::spawn_future_local`], so UI code never blocks on the network.

use std::fmt::Display;
use std::future::Future;

use anyhow::{Context, Result};
use flex_protocol::{
    artwork_path, health_path, item_path, items_path, libraries_path, scan_path, stream_path,
    Health, ItemQuery, Library, MediaItem, MediaKind, Page, ScanReport,
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

/// Small convenience wrapper around [`reqwest::Client`] plus the Tokio runtime
/// handle used to drive requests.
#[derive(Clone)]
pub struct ApiClient {
    base: String,
    http: reqwest::Client,
    handle: tokio::runtime::Handle,
}

impl ApiClient {
    pub fn new(base: impl Into<String>, handle: tokio::runtime::Handle) -> Self {
        let base = base.into();
        let base = base.trim_end_matches('/').to_string();
        Self {
            base,
            http: reqwest::Client::new(),
            handle,
        }
    }

    /// Base server URL without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Absolute URL for the range-capable byte stream of an item.
    pub fn stream_url(&self, id: i64) -> String {
        self.url(&stream_path(id))
    }

    /// Absolute URL for an item's artwork of the given type (`poster`/`backdrop`).
    pub fn artwork_url(&self, id: i64, kind: &str) -> String {
        format!("{}{}?type={}", self.base, artwork_path(id), kind)
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
            .http
            .get(self.url(&libraries_path()))
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
            format!("{}{}?{}", self.base, path, qs)
        };
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .context("request to /items failed")?;
        parse_json(resp, "/items").await
    }

    pub async fn item(&self, id: i64) -> Result<MediaItem> {
        let resp = self
            .http
            .get(self.url(&item_path(id)))
            .send()
            .await
            .with_context(|| format!("request to /items/{id} failed"))?;
        parse_json(resp, "item").await
    }

    pub async fn scan(&self) -> Result<ScanReport> {
        let resp = self
            .http
            .post(self.url(&scan_path()))
            .send()
            .await
            .context("request to /scan failed")?;
        parse_json(resp, "/scan").await
    }

    /// Fetch raw artwork bytes. Returns `None` when the server replies `404`
    /// (no artwork available).
    pub async fn artwork_bytes(&self, id: i64, kind: &str) -> Result<Option<Vec<u8>>> {
        let resp = self
            .http
            .get(self.artwork_url(id, kind))
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

async fn parse_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    what: &str,
) -> Result<T> {
    let status = resp.status();
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
}
