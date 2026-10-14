//! Configuration file and command line handling.

use std::path::PathBuf;

use clap::Parser;
use flex_protocol::LibraryKind;
use serde::Deserialize;

/// Default path of the configuration file.
pub const DEFAULT_CONFIG_PATH: &str = "./flex.toml";
/// Default SQLite database path.
pub const DEFAULT_DATABASE: &str = "./data/flex.db";

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
        }
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
}
