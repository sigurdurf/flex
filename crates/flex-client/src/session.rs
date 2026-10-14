//! Session persistence for the Flex client.
//!
//! Stores credentials in `~/.config/flex/client.json` with permissions `0600`
//! on Unix systems.

use std::fs::{self, File, Permissions};
use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub server_url: String,
    pub token: String,
    pub username: String,
}

pub fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("failed to determine config directory")?
        .join("flex");
    Ok(dir)
}

pub fn session_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("client.json"))
}

pub fn load() -> Result<Option<Session>> {
    let path = session_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let mut file = File::open(&path).context("failed to open session file")?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .context("failed to read session file")?;
    let session: Session =
        serde_json::from_str(&contents).context("failed to parse session file")?;
    Ok(Some(session))
}

pub fn save(session: &Session) -> Result<()> {
    let path = session_path()?;
    let dir = path
        .parent()
        .context("session path has no parent directory")?;
    fs::create_dir_all(dir).context("failed to create config directory")?;
    let json = serde_json::to_string_pretty(session).context("failed to serialize session")?;
    let mut file = File::create(&path).context("failed to create session file")?;
    #[cfg(unix)]
    {
        let perms = Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms).context("failed to set session file permissions")?;
    }
    file.write_all(json.as_bytes())
        .context("failed to write session file")?;
    Ok(())
}

pub fn clear() -> Result<()> {
    let path = session_path()?;
    if path.exists() {
        fs::remove_file(&path).context("failed to remove session file")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_serialization_roundtrip() {
        let session = Session {
            server_url: "http://127.0.0.1:8096".to_string(),
            token: "abc123".to_string(),
            username: "alice".to_string(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, session);
    }
}
