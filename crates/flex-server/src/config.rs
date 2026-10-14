//! Configuration file and command line handling.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use flex_protocol::LibraryKind;
use serde::Deserialize;

/// Default path of the configuration file.
pub const DEFAULT_CONFIG_PATH: &str = "./flex.toml";
/// Default SQLite database path.
pub const DEFAULT_DATABASE: &str = "./data/flex.db";
/// Default directory for transcode/playback session artifacts.
pub const DEFAULT_CACHE_DIR: &str = "./data/cache";
/// Default maximum number of concurrent remux/transcode sessions.
pub const DEFAULT_MAX_TRANSCODE_SESSIONS: usize = 2;
/// Default directory holding the static web UI.
pub const DEFAULT_WEB_DIR: &str = "./web";

/// Extensions treated as media when no `extensions` key is present in the config.
pub const DEFAULT_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "ts", "wmv", "flv", "mpg", "mpeg",
];

/// A single `[[libraries]]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    pub name: String,
    pub path: String,
    pub kind: LibraryKind,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            kind: LibraryKind::Other,
        }
    }
}

/// Full server configuration, mapping `config/flex.example.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub cache_dir: Option<String>,
    pub libraries: Vec<LibraryConfig>,
    pub extensions: Vec<String>,
    /// Whether remux/transcode (and thus `ffmpeg`) may be used at all.
    pub transcode_enabled: bool,
    /// Upper bound on concurrent remux/transcode sessions.
    pub max_transcode_sessions: usize,
    /// Directory containing the static web UI. When unset the server looks for a
    /// `web/` directory relative to the working directory and the executable.
    pub web_dir: Option<PathBuf>,
    /// OpenSubtitles.com API key. When unset the OpenSubtitles provider is not
    /// registered.
    pub opensubtitles_api_key: Option<String>,
    /// Optional OpenSubtitles account username.
    pub opensubtitles_username: Option<String>,
    /// Optional OpenSubtitles account password.
    pub opensubtitles_password: Option<String>,
    /// Register the deterministic in-memory `mock` subtitle provider. Testing
    /// only — never enable this in production.
    pub enable_mock_subtitles: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: flex_protocol::DEFAULT_HOST.to_string(),
            port: flex_protocol::DEFAULT_PORT,
            database: DEFAULT_DATABASE.to_string(),
            cache_dir: None,
            libraries: Vec::new(),
            extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            transcode_enabled: true,
            max_transcode_sessions: DEFAULT_MAX_TRANSCODE_SESSIONS,
            web_dir: None,
            opensubtitles_api_key: None,
            opensubtitles_username: None,
            opensubtitles_password: None,
            enable_mock_subtitles: false,
        }
    }
}

impl Config {
    /// Resolve the cache directory, falling back to [`DEFAULT_CACHE_DIR`].
    pub fn cache_path(&self) -> PathBuf {
        self.cache_dir
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(DEFAULT_CACHE_DIR)
            .into()
    }

    /// Resolve the static web UI directory.
    ///
    /// An explicit [`Config::web_dir`] always wins (even if it does not exist —
    /// serving simply 404s). Otherwise a `web/` directory next to the current
    /// working directory or next to the running executable is used. Returns
    /// `None` when no candidate exists, in which case static serving is
    /// disabled.
    pub fn web_path(&self) -> Option<PathBuf> {
        if let Some(dir) = self.web_dir.as_ref().filter(|p| !p.as_os_str().is_empty()) {
            return Some(dir.clone());
        }

        let mut candidates = vec![PathBuf::from(DEFAULT_WEB_DIR)];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                candidates.push(parent.join("web"));
            }
        }
        candidates.into_iter().find(|p| p.is_dir())
    }
}

/// Parsed command line arguments.
#[derive(Debug, Parser)]
#[command(
    name = "flex-server",
    version,
    about = "Headless Plex-alternative media server"
)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(long, env = "FLEX_CONFIG", default_value = DEFAULT_CONFIG_PATH)]
    pub config: PathBuf,
    /// Address to bind (overrides config).
    #[arg(long)]
    pub host: Option<String>,
    /// TCP port to bind (overrides config).
    #[arg(long)]
    pub port: Option<u16>,
    /// SQLite database file (overrides config).
    #[arg(long)]
    pub database: Option<PathBuf>,
    /// Cache directory for extracted artwork/subtitles (overrides config).
    #[arg(long = "cache-dir")]
    pub cache_dir: Option<PathBuf>,
    /// Run a library scan once at startup.
    #[arg(long = "scan-on-start")]
    pub scan_on_start: bool,
    /// Add a library as `NAME=PATH` (repeatable).
    #[arg(long = "library", value_name = "NAME=PATH")]
    pub libraries: Vec<String>,

    /// Administrative subcommand. When omitted the server runs normally.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Manage user accounts and their passwords.
    User(UserCommand),
}

/// `flex-server user ...`
#[derive(Debug, Clone, Args)]
pub struct UserCommand {
    #[command(subcommand)]
    pub action: UserAction,
}

/// Actions available under the `user` subcommand.
#[derive(Debug, Clone, Subcommand)]
pub enum UserAction {
    /// Create a new user.
    Add {
        /// Login name for the new user.
        username: String,
        /// Grant administrator privileges.
        #[arg(long)]
        admin: bool,
        /// Password (falls back to `$FLEX_PASSWORD`, then an interactive prompt).
        #[arg(long)]
        password: Option<String>,
    },
    /// List all users.
    List,
    /// Change a user's password.
    Passwd {
        /// Login name of the user.
        username: String,
        /// New password (falls back to `$FLEX_PASSWORD`, then a prompt).
        #[arg(long)]
        password: Option<String>,
    },
    /// Delete a user and revoke all of their device tokens.
    Delete {
        /// Login name of the user.
        username: String,
    },
}

/// Load configuration, applying CLI overrides on top of the file (or built-in
/// defaults when the file is absent).
pub fn load(cli: &Cli) -> anyhow::Result<(Config, bool)> {
    let path = &cli.config;
    let loaded = path.exists();
    let mut config = if loaded {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read config {}: {e}", path.display()))?;
        toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("failed to parse config {}: {e}", path.display()))?
    } else {
        tracing::warn!(
            "config file {} not found; falling back to built-in defaults",
            path.display()
        );
        Config::default()
    };

    if let Some(host) = &cli.host {
        config.host = host.clone();
    }
    if let Some(port) = cli.port {
        config.port = port;
    }
    if let Some(db) = &cli.database {
        config.database = db.to_string_lossy().into_owned();
    }
    if let Some(cache) = &cli.cache_dir {
        config.cache_dir = Some(cache.to_string_lossy().into_owned());
    }
    if config
        .opensubtitles_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        if let Ok(key) = std::env::var("FLEX_OPENSUBTITLES_API_KEY") {
            if !key.trim().is_empty() {
                config.opensubtitles_api_key = Some(key);
            }
        }
    }
    for spec in &cli.libraries {
        let (name, path) = spec.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("invalid --library value {spec:?}, expected NAME=PATH")
        })?;
        config.libraries.push(LibraryConfig {
            name: name.trim().to_string(),
            path: path.trim().to_string(),
            kind: LibraryKind::Other,
        });
    }

    Ok((config, loaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_example_extensions() {
        let cfg = Config::default();
        assert!(cfg.extensions.contains(&"mkv".to_string()));
        assert_eq!(cfg.port, flex_protocol::DEFAULT_PORT);
    }

    #[test]
    fn library_kind_deserializes_lowercase() {
        let cfg: Config = toml::from_str(
            r#"
            [[libraries]]
            name = "Movies"
            path = "/srv/media/movies"
            kind = "movies"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.libraries.len(), 1);
        assert_eq!(cfg.libraries[0].kind, LibraryKind::Movies);
        // missing keys fall back to defaults from `Default`.
        assert_eq!(cfg.port, flex_protocol::DEFAULT_PORT);
        assert!(cfg.extensions.contains(&"mp4".to_string()));
    }

    #[test]
    fn missing_kind_defaults_to_other() {
        let cfg: Config = toml::from_str(
            r#"
            [[libraries]]
            name = "Misc"
            path = "/srv/misc"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.libraries[0].kind, LibraryKind::Other);
    }

    #[test]
    fn cache_dir_defaults_and_overrides() {
        let cfg = Config::default();
        assert_eq!(cfg.cache_path(), PathBuf::from(DEFAULT_CACHE_DIR));
        assert!(cfg.transcode_enabled);
        assert_eq!(cfg.max_transcode_sessions, DEFAULT_MAX_TRANSCODE_SESSIONS);

        let cfg: Config = toml::from_str(
            r#"
            cache_dir = "/var/cache/flex"
            transcode_enabled = false
            max_transcode_sessions = 4
            "#,
        )
        .unwrap();
        assert_eq!(cfg.cache_path(), PathBuf::from("/var/cache/flex"));
        assert!(!cfg.transcode_enabled);
        assert_eq!(cfg.max_transcode_sessions, 4);
    }
}
