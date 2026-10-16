//! Online subtitle providers: search, download and sidecar installation.
//!
//! A [`SubtitleProvider`] abstracts a remote (or, for tests, in-memory) subtitle
//! source. Providers are collected in a [`SubtitleRegistry`] stored on
//! [`crate::api::AppState`]. The two HTTP handlers [`search`] and [`download`]
//! are wired into the bearer-protected router; see `docs/API.md` for the wire
//! contract.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{RawQuery, State};
use axum::Json;
use flex_protocol::{
    MediaItem, SubtitleDownloadRequest, SubtitleDownloadResult, SubtitleSearchQuery,
    SubtitleSearchResult,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::auth::AuthUser;
use crate::config::Config;
use crate::db;
use crate::error::AppError;
use crate::scanner;

/// Base URL of the OpenSubtitles.com REST API.
pub const OPENSUBTITLES_API_BASE: &str = "https://api.opensubtitles.com/api/v1";
/// Default language used when neither the request nor the provider names one.
pub const DEFAULT_LANGUAGE: &str = "und";

/// A future returned by [`SubtitleProvider`] methods.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The on-disk/wire format of a downloaded subtitle payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Vtt,
    Sub,
}

impl SubtitleFormat {
    /// Guess a format from a file extension (case-insensitive).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "srt" => Some(Self::Srt),
            "vtt" | "webvtt" => Some(Self::Vtt),
            "sub" => Some(Self::Sub),
            _ => None,
        }
    }

    /// The canonical lowercase extension.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Sub => "sub",
        }
    }
}

/// Raw bytes downloaded from a provider plus a format hint.
#[derive(Debug, Clone)]
pub struct SubtitlePayload {
    pub bytes: Vec<u8>,
    pub format: SubtitleFormat,
}

impl SubtitlePayload {
    pub fn new(bytes: Vec<u8>, format: SubtitleFormat) -> Self {
        Self { bytes, format }
    }
}

/// Errors a provider may surface. The HTTP layer maps these to status codes.
#[derive(Debug, Clone)]
pub enum SubtitleError {
    /// The provider does not know the requested subtitle id.
    NotFound(String),
    /// An upstream/network/rate-limit failure.
    Upstream(String),
    /// A local failure (e.g. format conversion or client construction).
    Internal(String),
}

impl std::fmt::Display for SubtitleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "subtitle not found: {msg}"),
            Self::Upstream(msg) => write!(f, "subtitle provider error: {msg}"),
            Self::Internal(msg) => write!(f, "subtitle internal error: {msg}"),
        }
    }
}

impl std::error::Error for SubtitleError {}

impl From<SubtitleError> for AppError {
    fn from(err: SubtitleError) -> Self {
        match err {
            SubtitleError::NotFound(msg) => AppError::not_found(msg),
            SubtitleError::Upstream(msg) => AppError::bad_gateway(msg),
            SubtitleError::Internal(msg) => AppError::internal(msg),
        }
    }
}

/// A source of subtitles that can be searched and downloaded.
///
/// The methods return boxed futures so the trait stays object-safe without an
/// extra `async-trait` dependency.
pub trait SubtitleProvider: Send + Sync {
    /// Stable identifier used in `provider` fields and requests (e.g. `mock`).
    fn name(&self) -> &str;

    /// Search the provider for subtitles matching `query`.
    fn search<'a>(
        &'a self,
        query: &'a SubtitleSearchQuery,
    ) -> ProviderFuture<'a, Result<Vec<SubtitleSearchResult>, SubtitleError>>;

    /// Download a specific subtitle by its provider id.
    fn download<'a>(
        &'a self,
        id: &'a str,
        language: Option<&'a str>,
    ) -> ProviderFuture<'a, Result<SubtitlePayload, SubtitleError>>;
}

// --- Mock provider ----------------------------------------------------------

/// A deterministic in-memory provider used by tests and the E2E harness.
///
/// Registered only when `enable_mock_subtitles = true`. Never enable this in a
/// production deployment.
pub struct MockProvider;

impl MockProvider {
    pub fn new() -> Self {
        Self
    }

    /// Ids the mock provider knows about.
    pub const IDS: [&'static str; 2] = ["mock-1", "mock-2"];

    /// The canned SRT body returned for every successful download.
    pub const SRT: &'static [u8] = b"1\n\
00:00:01,000 --> 00:00:03,000\n\
Hello from the Flex mock subtitle provider.\n\
\n\
2\n\
00:00:04,000 --> 00:00:06,000\n\
Second line.\n";
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SubtitleProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn search<'a>(
        &'a self,
        query: &'a SubtitleSearchQuery,
    ) -> ProviderFuture<'a, Result<Vec<SubtitleSearchResult>, SubtitleError>> {
        Box::pin(async move {
            let language = query
                .languages
                .first()
                .cloned()
                .unwrap_or_else(|| "en".to_string());
            let title = query
                .query
                .clone()
                .unwrap_or_else(|| "Mock Title".to_string());
            Ok(vec![
                SubtitleSearchResult {
                    provider: "mock".to_string(),
                    id: Self::IDS[0].to_string(),
                    language: language.clone(),
                    title: Some(title.clone()),
                    release: Some("mock.release.1080p".to_string()),
                    download_count: Some(1000),
                    rating: Some(8.5),
                },
                SubtitleSearchResult {
                    provider: "mock".to_string(),
                    id: Self::IDS[1].to_string(),
                    language,
                    title: Some(format!("{title} (alt)")),
                    release: Some("mock.release.web".to_string()),
                    download_count: Some(42),
                    rating: Some(7.0),
                },
            ])
        })
    }

    fn download<'a>(
        &'a self,
        id: &'a str,
        _language: Option<&'a str>,
    ) -> ProviderFuture<'a, Result<SubtitlePayload, SubtitleError>> {
        Box::pin(async move {
            if Self::IDS.contains(&id) {
                Ok(SubtitlePayload::new(
                    Self::SRT.to_vec(),
                    SubtitleFormat::Srt,
                ))
            } else {
                Err(SubtitleError::NotFound(format!(
                    "mock subtitle {id} not found"
                )))
            }
        })
    }
}

// --- OpenSubtitles provider -------------------------------------------------

/// OpenSubtitles.com REST API provider.
pub struct OpenSubtitlesProvider {
    client: reqwest::Client,
    api_key: String,
    /// Optional account credentials, used to obtain a download token.
    credentials: Option<(String, String)>,
    /// Cached bearer token from `/login`, when credentials are configured.
    token: tokio::sync::Mutex<Option<String>>,
}

impl OpenSubtitlesProvider {
    /// Build a provider. `api_key` must be non-empty.
    pub fn new(
        api_key: impl Into<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<Self, SubtitleError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("flex-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| SubtitleError::Internal(format!("failed to build HTTP client: {e}")))?;
        let credentials = match (username, password) {
            (Some(username), Some(password)) if !username.trim().is_empty() => {
                Some((username, password))
            }
            _ => None,
        };
        Ok(Self {
            client,
            api_key: api_key.into(),
            credentials,
            token: tokio::sync::Mutex::new(None),
        })
    }

    /// Return a cached download token, logging in on first use.
    async fn auth_token(&self) -> Result<Option<String>, SubtitleError> {
        let Some((username, password)) = self.credentials.as_ref() else {
            return Ok(None);
        };
        let mut guard = self.token.lock().await;
        if let Some(token) = guard.as_ref() {
            return Ok(Some(token.clone()));
        }

        #[derive(Deserialize)]
        struct LoginResponse {
            #[serde(default)]
            token: Option<String>,
        }
        let response = self
            .client
            .post(format!("{OPENSUBTITLES_API_BASE}/login"))
            .header("Api-Key", &self.api_key)
            .json(&serde_json::json!({ "username": username, "password": password }))
            .send()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SubtitleError::Upstream(format!(
                "OpenSubtitles login failed with HTTP {status}"
            )));
        }
        let login: LoginResponse = response
            .json()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;
        let token = login.token.ok_or_else(|| {
            SubtitleError::Upstream("OpenSubtitles login returned no token".into())
        })?;
        *guard = Some(token.clone());
        Ok(Some(token))
    }

    async fn search_impl(
        &self,
        query: &SubtitleSearchQuery,
    ) -> Result<Vec<SubtitleSearchResult>, SubtitleError> {
        let text = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| SubtitleError::Upstream("search query must not be empty".into()))?;

        let mut request = self
            .client
            .get(format!("{OPENSUBTITLES_API_BASE}/subtitles"))
            .header("Api-Key", &self.api_key)
            .query(&[("query", text)]);
        let languages = query.languages.join(",");
        if !languages.is_empty() {
            request = request.query(&[("languages", languages)]);
        }

        let response = request
            .send()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SubtitleError::Upstream(
                "OpenSubtitles rate limit exceeded".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(SubtitleError::Upstream(format!(
                "OpenSubtitles search failed with HTTP {status}"
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;
        map_open_subtitles_search(&body)
    }

    async fn download_impl(&self, id: &str) -> Result<SubtitlePayload, SubtitleError> {
        let file_id: i64 = id
            .trim()
            .parse()
            .map_err(|_| SubtitleError::NotFound(format!("invalid OpenSubtitles id {id:?}")))?;

        let mut request = self
            .client
            .post(format!("{OPENSUBTITLES_API_BASE}/download"))
            .header("Api-Key", &self.api_key);
        if let Some(token) = self.auth_token().await? {
            request = request.bearer_auth(token);
        }
        let response = request
            .json(&serde_json::json!({ "file_id": file_id }))
            .send()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SubtitleError::NotFound(format!(
                "OpenSubtitles subtitle {id} not found"
            )));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SubtitleError::Upstream(
                "OpenSubtitles rate limit exceeded".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(SubtitleError::Upstream(format!(
                "OpenSubtitles download failed with HTTP {status}"
            )));
        }

        #[derive(Deserialize)]
        struct DownloadResponse {
            #[serde(default)]
            link: Option<String>,
            #[serde(default)]
            file_name: Option<String>,
        }
        let download: DownloadResponse = response
            .json()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;
        let link = download.link.ok_or_else(|| {
            SubtitleError::Upstream("OpenSubtitles returned no download link".into())
        })?;

        let format = download
            .file_name
            .as_deref()
            .and_then(|name| Path::new(name).extension().and_then(|e| e.to_str()))
            .and_then(SubtitleFormat::from_extension)
            .unwrap_or(SubtitleFormat::Srt);

        let bytes = self
            .client
            .get(&link)
            .send()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?
            .error_for_status()
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| SubtitleError::Upstream(e.to_string()))?;

        Ok(SubtitlePayload::new(bytes.to_vec(), format))
    }
}

impl SubtitleProvider for OpenSubtitlesProvider {
    fn name(&self) -> &str {
        "opensubtitles"
    }

    fn search<'a>(
        &'a self,
        query: &'a SubtitleSearchQuery,
    ) -> ProviderFuture<'a, Result<Vec<SubtitleSearchResult>, SubtitleError>> {
        Box::pin(self.search_impl(query))
    }

    fn download<'a>(
        &'a self,
        id: &'a str,
        _language: Option<&'a str>,
    ) -> ProviderFuture<'a, Result<SubtitlePayload, SubtitleError>> {
        Box::pin(self.download_impl(id))
    }
}

// --- OpenSubtitles JSON mapping --------------------------------------------

#[derive(Debug, Deserialize)]
struct OsSearchResponse {
    #[serde(default)]
    data: Vec<OsData>,
}

#[derive(Debug, Deserialize, Default)]
struct OsData {
    #[serde(default)]
    attributes: OsAttributes,
}

#[derive(Debug, Deserialize, Default)]
struct OsAttributes {
    #[serde(default)]
    subtitle_id: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    download_count: Option<u64>,
    #[serde(default)]
    ratings: Option<f32>,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    feature_details: Option<OsFeatureDetails>,
    #[serde(default)]
    files: Vec<OsFile>,
}

#[derive(Debug, Deserialize, Default)]
struct OsFeatureDetails {
    #[serde(default)]
    movie_name: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OsFile {
    #[serde(default)]
    file_id: Option<i64>,
}

/// Map an OpenSubtitles `/subtitles` JSON body into protocol search results.
///
/// Kept as a free function so it can be unit-tested against a canned fixture
/// without touching the network.
pub fn map_open_subtitles_search(body: &str) -> Result<Vec<SubtitleSearchResult>, SubtitleError> {
    let parsed: OsSearchResponse = serde_json::from_str(body)
        .map_err(|e| SubtitleError::Upstream(format!("invalid OpenSubtitles response: {e}")))?;

    let results = parsed
        .data
        .into_iter()
        .filter_map(|entry| {
            let attrs = entry.attributes;
            let file = attrs.files.into_iter().next();
            let id = file
                .as_ref()
                .and_then(|f| f.file_id)
                .map(|id| id.to_string())
                .or_else(|| attrs.subtitle_id.clone())?;
            let title = attrs
                .feature_details
                .as_ref()
                .and_then(|d| d.movie_name.clone().or_else(|| d.title.clone()));
            Some(SubtitleSearchResult {
                provider: "opensubtitles".to_string(),
                id,
                language: attrs.language.unwrap_or_default(),
                title,
                release: attrs.release,
                download_count: attrs.download_count,
                rating: attrs.ratings,
            })
        })
        .collect();

    Ok(results)
}

// --- Registry ---------------------------------------------------------------

/// The set of configured [`SubtitleProvider`]s.
#[derive(Default)]
pub struct SubtitleRegistry {
    providers: Vec<Arc<dyn SubtitleProvider>>,
}

impl SubtitleRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the registry implied by `config`.
    pub fn from_config(config: &Config) -> Self {
        let mut registry = Self::new();

        if let Some(key) = config
            .opensubtitles_api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            match OpenSubtitlesProvider::new(
                key,
                config.opensubtitles_username.clone(),
                config.opensubtitles_password.clone(),
            ) {
                Ok(provider) => {
                    tracing::info!("registered OpenSubtitles subtitle provider");
                    registry.register(Arc::new(provider));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to register OpenSubtitles provider")
                }
            }
        }

        if config.enable_mock_subtitles {
            tracing::warn!("registering mock subtitle provider (testing only)");
            registry.register(Arc::new(MockProvider::new()));
        }

        if !registry.is_empty() {
            tracing::info!(providers = ?registry.names(), "subtitle providers configured");
        }
        registry
    }

    /// Add a provider.
    pub fn register(&mut self, provider: Arc<dyn SubtitleProvider>) {
        self.providers.push(provider);
    }

    /// Look up a provider by its [`SubtitleProvider::name`].
    pub fn get(&self, name: &str) -> Option<Arc<dyn SubtitleProvider>> {
        self.providers.iter().find(|p| p.name() == name).cloned()
    }

    /// Whether any provider is registered.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Iterate over the registered providers.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn SubtitleProvider>> {
        self.providers.iter()
    }

    /// The names of the registered providers, in registration order.
    pub fn names(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }
}

// --- Pure helpers -----------------------------------------------------------

/// Derive a provider search query from an indexed item.
///
/// The title is used verbatim; the release year is appended in parentheses when
/// known, and a `SxxExx` token is appended when both `season` and `episode` are
/// supplied.
pub fn derive_query(item: &MediaItem, season: Option<u32>, episode: Option<u32>) -> String {
    let mut query = item.title.clone();
    if let Some(year) = item.year {
        query.push_str(&format!(" ({year})"));
    }
    if let (Some(season), Some(episode)) = (season, episode) {
        query.push_str(&format!(" S{season:02}E{episode:02}"));
    }
    query
}

/// The sidecar path for `language` next to a media file: `<stem>.<lang>.vtt`.
pub fn sidecar_path(media: &Path, language: &str) -> PathBuf {
    let stem = media
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("subtitle");
    let parent = media.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}.{language}.vtt"))
}

/// Normalise a language tag, falling back to [`DEFAULT_LANGUAGE`].
pub fn normalize_language(language: Option<&str>) -> String {
    language
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(DEFAULT_LANGUAGE)
        .to_string()
}

/// Convert arbitrary subtitle bytes to WebVTT.
///
/// WebVTT input is returned unchanged; SRT/SUB input is transcoded with
/// `ffmpeg`. The intermediate file is written to a unique temp path and removed
/// afterwards.
pub async fn convert_to_vtt(
    bytes: Vec<u8>,
    format: SubtitleFormat,
) -> Result<Vec<u8>, SubtitleError> {
    if format == SubtitleFormat::Vtt {
        return Ok(bytes);
    }

    let tmp = std::env::temp_dir().join(format!(
        "flex-subtitle-{}.{}",
        uuid::Uuid::new_v4(),
        format.as_str()
    ));
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| SubtitleError::Internal(format!("failed to write temp subtitle: {e}")))?;

    let output = tokio::process::Command::new("ffmpeg")
        .args(["-v", "quiet", "-i"])
        .arg(&tmp)
        .args(["-f", "webvtt", "-"])
        .output()
        .await;
    let _ = tokio::fs::remove_file(&tmp).await;

    let output =
        output.map_err(|e| SubtitleError::Internal(format!("failed to run ffmpeg: {e}")))?;
    if !output.status.success() {
        return Err(SubtitleError::Internal(
            "ffmpeg failed to convert subtitle to WebVTT".to_string(),
        ));
    }
    Ok(output.stdout)
}

/// Write a WebVTT file to `path`, creating parent directories as needed.
pub async fn write_sidecar(path: &Path, vtt: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, vtt).await
}

// --- HTTP handlers ----------------------------------------------------------

/// Query string for `GET /subtitles/search`.
///
/// `languages` accepts both repeated keys (`?languages=en&languages=fr`) and a
/// single comma-separated value (`?languages=en,fr`).
#[derive(Debug, Default, Deserialize)]
pub struct SubtitleSearchParams {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
}

impl SubtitleSearchParams {
    /// Flatten comma-separated language values, trimming blanks.
    pub fn normalized_languages(&self) -> Vec<String> {
        self.languages
            .iter()
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
            .collect()
    }
}

/// Percent-decode one query component (`+` means space).
fn decode_component(value: &str) -> String {
    percent_encoding::percent_decode_str(&value.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

/// Parse the raw query string for `/subtitles/search`.
///
/// Parsed by hand (rather than via `serde_urlencoded`) so `languages` may be
/// repeated. Unknown keys are ignored; malformed integers are a `400`.
pub fn parse_search_params(raw: Option<&str>) -> Result<SubtitleSearchParams, AppError> {
    let mut params = SubtitleSearchParams::default();
    let Some(raw) = raw else {
        return Ok(params);
    };

    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key);
        let value = decode_component(value);
        match key.as_str() {
            "query" => params.query = Some(value),
            "languages" => params.languages.push(value),
            "item_id" => {
                params.item_id = Some(
                    value
                        .parse()
                        .map_err(|_| AppError::bad_request(format!("invalid item_id {value:?}")))?,
                )
            }
            "season" => {
                params.season = Some(
                    value
                        .parse()
                        .map_err(|_| AppError::bad_request(format!("invalid season {value:?}")))?,
                )
            }
            "episode" => {
                params.episode = Some(
                    value
                        .parse()
                        .map_err(|_| AppError::bad_request(format!("invalid episode {value:?}")))?,
                )
            }
            _ => {}
        }
    }

    Ok(params)
}

static LOGGED_NO_PROVIDERS: AtomicBool = AtomicBool::new(false);

/// `GET /subtitles/search`
pub async fn search(
    _auth: AuthUser,
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<SubtitleSearchResult>>, AppError> {
    let registry = &state.subtitles;
    if registry.is_empty() {
        if !LOGGED_NO_PROVIDERS.swap(true, Ordering::Relaxed) {
            tracing::info!("no subtitle providers configured; subtitle search returns no results");
        }
        return Ok(Json(Vec::new()));
    }

    let params = parse_search_params(raw.as_deref())?;
    let languages = params.normalized_languages();
    let mut query = SubtitleSearchQuery {
        query: params
            .query
            .clone()
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty()),
        languages,
        item_id: params.item_id,
        season: params.season,
        episode: params.episode,
    };

    if query.query.is_none() {
        match query.item_id {
            Some(item_id) => {
                let item = db::get_item(&state.pool, item_id).await?.ok_or_else(|| {
                    AppError::not_found(format!("media item {item_id} not found"))
                })?;
                query.query = Some(derive_query(&item, query.season, query.episode));
            }
            None => {
                return Err(AppError::bad_request(
                    "either `query` or `item_id` must be supplied",
                ))
            }
        }
    }

    let mut results = Vec::new();
    let mut last_error: Option<SubtitleError> = None;
    for provider in registry.iter() {
        match provider.search(&query).await {
            Ok(mut found) => results.append(&mut found),
            Err(err) => {
                tracing::warn!(provider = provider.name(), error = %err, "subtitle search failed");
                last_error = Some(err);
            }
        }
    }

    if results.is_empty() {
        if let Some(err) = last_error {
            return Err(err.into());
        }
    }

    Ok(Json(results))
}

/// `POST /subtitles/download`
///
/// The body is decoded manually via `Result<Json<_>, _>` so a missing or
/// malformed body is a `400` rather than axum's default `422`.
pub async fn download(
    _auth: AuthUser,
    State(state): State<AppState>,
    body: Result<Json<SubtitleDownloadRequest>, JsonRejection>,
) -> Result<Json<SubtitleDownloadResult>, AppError> {
    let Json(request) = body.map_err(|rejection| AppError::bad_request(rejection.body_text()))?;
    let provider = state.subtitles.get(&request.provider).ok_or_else(|| {
        AppError::bad_request(format!("unknown subtitle provider {:?}", request.provider))
    })?;

    let language = normalize_language(request.language.as_deref());
    let payload = provider
        .download(&request.id, Some(&language))
        .await
        .map_err(AppError::from)?;
    let vtt = convert_to_vtt(payload.bytes, payload.format)
        .await
        .map_err(AppError::from)?;

    let mut installed_path: Option<String> = None;
    let mut item_id = 0i64;

    if let Some(id) = request.item_id {
        item_id = id;
        let item = db::get_item(&state.pool, id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))?;

        let target = sidecar_path(Path::new(&item.file_path), &language);
        match write_sidecar(&target, &vtt).await {
            Ok(()) => {
                installed_path = Some(target.to_string_lossy().into_owned());
                match scanner::reindex_item(&state.pool, id).await {
                    Ok(()) => {
                        tracing::info!(item_id = id, path = %target.display(), "installed subtitle sidecar")
                    }
                    Err(err) => tracing::warn!(
                        item_id = id,
                        error = %err,
                        "subtitle installed but re-indexing the item failed"
                    ),
                }
            }
            Err(err) => tracing::warn!(
                item_id = id,
                path = %target.display(),
                error = %err,
                "media directory not writable; subtitle not installed"
            ),
        }
    }

    Ok(Json(SubtitleDownloadResult {
        item_id,
        path: installed_path,
        language,
        format: "vtt".to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flex_protocol::MediaKind;

    fn item(title: &str, year: Option<i32>) -> MediaItem {
        MediaItem {
            id: 1,
            library_id: 1,
            kind: MediaKind::Movie,
            title: title.to_string(),
            sort_title: title.to_lowercase(),
            year,
            overview: None,
            file_path: "/media/Movie.2020.mkv".to_string(),
            file_size: 0,
            container: Some("matroska".to_string()),
            duration_ms: None,
            video: None,
            audio: Vec::new(),
            subtitles: Vec::new(),
            added_at: 0,
            updated_at: 0,
            poster_url: None,
            backdrop_url: None,
            series_id: None,
            season: None,
            episode: None,
        }
    }

    #[test]
    fn mock_registration_is_gated_by_config() {
        let config = Config::default();
        assert!(SubtitleRegistry::from_config(&config).is_empty());

        let config = Config {
            enable_mock_subtitles: true,
            ..Config::default()
        };
        let registry = SubtitleRegistry::from_config(&config);
        assert_eq!(registry.names(), vec!["mock".to_string()]);
        assert!(registry.get("mock").is_some());
    }

    #[test]
    fn unknown_provider_is_none() {
        let config = Config {
            enable_mock_subtitles: true,
            ..Config::default()
        };
        let registry = SubtitleRegistry::from_config(&config);
        assert!(registry.get("does-not-exist").is_none());
    }

    #[test]
    fn opensubtitles_registered_only_with_key() {
        let config = Config {
            opensubtitles_api_key: Some("   ".to_string()),
            ..Config::default()
        };
        assert!(SubtitleRegistry::from_config(&config).is_empty());

        let config = Config {
            opensubtitles_api_key: Some("secret-key".to_string()),
            ..Config::default()
        };
        let registry = SubtitleRegistry::from_config(&config);
        assert_eq!(registry.names(), vec!["opensubtitles".to_string()]);
    }

    #[test]
    fn derives_query_from_item() {
        assert_eq!(
            derive_query(&item("Blade Runner", Some(1982)), None, None),
            "Blade Runner (1982)"
        );
        assert_eq!(
            derive_query(&item("Some Show", None), None, None),
            "Some Show"
        );
        assert_eq!(
            derive_query(&item("Some Show", Some(2020)), Some(1), Some(2)),
            "Some Show (2020) S01E02"
        );
        assert_eq!(
            derive_query(&item("Some Show", None), Some(12), Some(7)),
            "Some Show S12E07"
        );
        // Season without episode does not append a token.
        assert_eq!(
            derive_query(&item("Some Show", None), Some(1), None),
            "Some Show"
        );
    }

    #[test]
    fn sidecar_path_uses_stem_and_language() {
        let path = sidecar_path(Path::new("/media/Movie.2020.mkv"), "en");
        assert_eq!(path, PathBuf::from("/media/Movie.2020.en.vtt"));
        assert_eq!(
            sidecar_path(Path::new("/media/tv/Show.S01E02.1080p.mkv"), "pt-br"),
            PathBuf::from("/media/tv/Show.S01E02.1080p.pt-br.vtt")
        );
    }

    #[test]
    fn normalizes_languages() {
        let params = SubtitleSearchParams {
            languages: vec!["en,fr".to_string(), " de ".to_string(), "".to_string()],
            ..Default::default()
        };
        assert_eq!(params.normalized_languages(), vec!["en", "fr", "de"]);
        assert_eq!(normalize_language(None), "und");
        assert_eq!(normalize_language(Some("  ")), "und");
        assert_eq!(normalize_language(Some("en")), "en");
    }

    #[test]
    fn maps_opensubtitles_search_json() {
        let fixture = r#"{
            "total_count": 2,
            "data": [
                {
                    "id": "100",
                    "attributes": {
                        "subtitle_id": "100",
                        "language": "en",
                        "download_count": 1234,
                        "ratings": 8.5,
                        "release": "Blade.Runner.1982.1080p",
                        "feature_details": { "movie_name": "Blade Runner", "year": 1982 },
                        "files": [ { "file_id": 555, "file_name": "blade.srt" } ]
                    }
                },
                {
                    "id": "101",
                    "attributes": {
                        "language": "fr",
                        "files": [ { "file_id": 556 } ]
                    }
                }
            ]
        }"#;
        let results = map_open_subtitles_search(fixture).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].provider, "opensubtitles");
        assert_eq!(results[0].id, "555");
        assert_eq!(results[0].language, "en");
        assert_eq!(results[0].title.as_deref(), Some("Blade Runner"));
        assert_eq!(
            results[0].release.as_deref(),
            Some("Blade.Runner.1982.1080p")
        );
        assert_eq!(results[0].download_count, Some(1234));
        assert_eq!(results[0].rating, Some(8.5));
        assert_eq!(results[1].id, "556");
        assert_eq!(results[1].language, "fr");
        assert_eq!(results[1].title, None);
    }

    #[test]
    fn opensubtitles_mapping_rejects_garbage() {
        assert!(map_open_subtitles_search("not json").is_err());
    }

    #[test]
    fn format_from_extension() {
        assert_eq!(
            SubtitleFormat::from_extension("SRT"),
            Some(SubtitleFormat::Srt)
        );
        assert_eq!(
            SubtitleFormat::from_extension(".vtt"),
            Some(SubtitleFormat::Vtt)
        );
        assert_eq!(
            SubtitleFormat::from_extension("sub"),
            Some(SubtitleFormat::Sub)
        );
        assert_eq!(SubtitleFormat::from_extension("ass"), None);
    }

    #[tokio::test]
    async fn vtt_passthrough_is_unchanged() {
        let body = b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nhi\n".to_vec();
        let out = convert_to_vtt(body.clone(), SubtitleFormat::Vtt)
            .await
            .unwrap();
        assert_eq!(out, body);
    }

    #[tokio::test]
    async fn converts_srt_to_vtt_and_writes_sidecar() {
        let srt = b"1\n00:00:01,000 --> 00:00:03,000\nHello.\n\n".to_vec();
        let vtt = convert_to_vtt(srt, SubtitleFormat::Srt).await.unwrap();
        let text = String::from_utf8_lossy(&vtt);
        assert!(text.starts_with("WEBVTT"), "got: {text:?}");
        assert!(text.contains("Hello."));

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("Movie.2020.mkv");
        let target = sidecar_path(&media, "en");
        write_sidecar(&target, &vtt).await.unwrap();
        let written = tokio::fs::read(&target).await.unwrap();
        assert_eq!(written, vtt);
        assert_eq!(target.file_name().unwrap(), "Movie.2020.en.vtt");
    }

    #[tokio::test]
    async fn mock_provider_roundtrip() {
        let provider = MockProvider::new();
        let query = SubtitleSearchQuery {
            query: Some("Blade Runner".to_string()),
            languages: vec!["en".to_string()],
            ..Default::default()
        };
        let results = provider.search(&query).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "mock-1");

        let payload = provider.download(&results[0].id, Some("en")).await.unwrap();
        assert_eq!(payload.format, SubtitleFormat::Srt);
        assert!(!payload.bytes.is_empty());

        assert!(matches!(
            provider.download("nope", Some("en")).await,
            Err(SubtitleError::NotFound(_))
        ));
    }

    #[test]
    fn query_params_parse_repeated_and_comma_languages() {
        let params = parse_search_params(Some("item_id=1&languages=en&languages=fr")).unwrap();
        assert_eq!(params.item_id, Some(1));
        assert_eq!(params.normalized_languages(), vec!["en", "fr"]);

        let params = parse_search_params(Some("query=Blade%20Runner&languages=en,fr,de")).unwrap();
        assert_eq!(params.query.as_deref(), Some("Blade Runner"));
        assert_eq!(params.normalized_languages(), vec!["en", "fr", "de"]);

        assert!(parse_search_params(None).unwrap().item_id.is_none());
    }

    #[test]
    fn query_params_reject_malformed_ints() {
        assert!(matches!(
            parse_search_params(Some("item_id=abc")),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            parse_search_params(Some("season=x")),
            Err(AppError::BadRequest(_))
        ));
    }

    async fn test_state() -> AppState {
        let config = Config {
            enable_mock_subtitles: true,
            ..Config::default()
        };
        AppState {
            pool: db::test_pool().await,
            rate_limiter: Arc::new(crate::auth::LoginRateLimiter::new()),
            sessions: Arc::new(crate::playback::SessionManager::new(
                std::env::temp_dir(),
                2,
            )),
            transcode: Arc::new(crate::transcode::TranscodeCapabilities::default()),
            subtitles: Arc::new(SubtitleRegistry::from_config(&config)),
            config: Arc::new(config),
        }
    }

    fn dummy_auth() -> AuthUser {
        AuthUser {
            user: flex_protocol::User {
                id: 1,
                username: "tester".to_string(),
                role: flex_protocol::UserRole::Admin,
                created_at: 0,
            },
            device_id: 1,
        }
    }

    async fn seed_item(state: &AppState, media_path: &Path) -> i64 {
        let library_id = db::upsert_library(
            &state.pool,
            "Movies",
            media_path.parent().unwrap().to_str().unwrap(),
            flex_protocol::LibraryKind::Movies,
        )
        .await
        .unwrap();
        let new_item = crate::model::NewMediaItem {
            library_id,
            kind: MediaKind::Movie,
            title: "Blade Runner".to_string(),
            sort_title: "blade runner".to_string(),
            year: Some(1982),
            overview: None,
            file_path: media_path.to_string_lossy().into_owned(),
            file_size: 0,
            container: Some("matroska".to_string()),
            duration_ms: None,
            streams: crate::model::StreamData::default(),
            sidecars: Vec::new(),
            poster_path: None,
            backdrop_path: None,
            series_id: None,
            season: None,
            episode: None,
        };
        db::upsert_media(&state.pool, &new_item).await.unwrap().0
    }

    #[tokio::test]
    async fn search_handler_derives_query_from_item() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("Blade Runner (1982).mkv");
        let state = test_state().await;
        let id = seed_item(&state, &media).await;

        let Json(results) = search(
            dummy_auth(),
            State(state),
            RawQuery(Some(format!("item_id={id}&languages=en"))),
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].provider, "mock");
        assert_eq!(results[0].title.as_deref(), Some("Blade Runner (1982)"));
    }

    #[tokio::test]
    async fn search_handler_requires_query_or_item() {
        let state = test_state().await;
        let result = search(dummy_auth(), State(state), RawQuery(None)).await;
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[tokio::test]
    async fn search_with_no_providers_returns_empty() {
        let state = AppState {
            subtitles: Arc::new(SubtitleRegistry::new()),
            ..test_state().await
        };
        let Json(results) = search(
            dummy_auth(),
            State(state),
            RawQuery(Some("query=anything".to_string())),
        )
        .await
        .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn download_handler_installs_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("Blade Runner (1982).mkv");
        tokio::fs::write(&media, b"not a real video").await.unwrap();
        let state = test_state().await;
        let id = seed_item(&state, &media).await;

        let request = SubtitleDownloadRequest {
            provider: "mock".to_string(),
            id: "mock-1".to_string(),
            language: Some("en".to_string()),
            item_id: Some(id),
        };
        let Json(result) = download(dummy_auth(), State(state), Ok(Json(request)))
            .await
            .unwrap();
        assert_eq!(result.item_id, id);
        assert_eq!(result.language, "en");
        assert_eq!(result.format, "vtt");
        let path = result.path.expect("sidecar should be installed");
        assert!(path.ends_with("Blade Runner (1982).en.vtt"));
        let bytes = tokio::fs::read(&path).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).starts_with("WEBVTT"));
    }

    #[tokio::test]
    async fn download_handler_rejects_unknown_provider() {
        let state = test_state().await;
        let request = SubtitleDownloadRequest {
            provider: "nope".to_string(),
            id: "x".to_string(),
            language: None,
            item_id: None,
        };
        let result = download(dummy_auth(), State(state), Ok(Json(request))).await;
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[tokio::test]
    async fn download_handler_missing_item_is_404() {
        let state = test_state().await;
        let request = SubtitleDownloadRequest {
            provider: "mock".to_string(),
            id: "mock-1".to_string(),
            language: Some("en".to_string()),
            item_id: Some(999),
        };
        let result = download(dummy_auth(), State(state), Ok(Json(request))).await;
        assert!(matches!(result, Err(AppError::NotFound(_))));
    }

    #[tokio::test]
    async fn download_handler_without_item_returns_zero() {
        let state = test_state().await;
        let request = SubtitleDownloadRequest {
            provider: "mock".to_string(),
            id: "mock-1".to_string(),
            language: Some("en".to_string()),
            item_id: None,
        };
        let Json(result) = download(dummy_auth(), State(state), Ok(Json(request)))
            .await
            .unwrap();
        assert_eq!(result.item_id, 0);
        assert!(result.path.is_none());
        assert_eq!(result.format, "vtt");
    }

    #[tokio::test]
    async fn download_handler_unknown_mock_id_is_404() {
        let state = test_state().await;
        let request = SubtitleDownloadRequest {
            provider: "mock".to_string(),
            id: "mock-999".to_string(),
            language: Some("en".to_string()),
            item_id: None,
        };
        let result = download(dummy_auth(), State(state), Ok(Json(request))).await;
        assert!(matches!(result, Err(AppError::NotFound(_))));
    }
}
