//! Playback engine.
//!
//! Two backends are supported:
//!
//! * [`PlayerBackend::Libmpv`] — libmpv is embedded directly into the GTK
//!   window using the native X11 window id (`wid`). This is the primary path
//!   and works under XWayland.
//! * [`PlayerBackend::External`] — when no X11 surface is available (a pure
//!   Wayland session), an external `mpv --force-window=yes <url>` process is
//!   spawned instead. This is a pragmatic fallback: libmpv cannot embed into an
//!   arbitrary Wayland surface without a render API integration.
//!
//! # Embedding notes
//!
//! Because `wid` refers to the *top-level* X11 window, libmpv paints over the
//! whole GTK window. The player page therefore treats the video as full-window;
//! the transport controls still function but may be hidden behind the video
//! surface while embedded. Keyboard shortcuts (`Space`, `←`/`→`, `f`) remain a
//! reliable way to control playback in that mode.

use std::process::{Child, Command, Stdio};

use anyhow::Result;
use gtk4::prelude::*;
use libmpv2::Mpv;

/// Which engine is currently driving playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerBackend {
    Libmpv,
    External,
}

/// A thin, uniform wrapper over the two playback backends.
pub struct Player {
    backend: PlayerBackend,
    mpv: Option<Mpv>,
    external: Option<Child>,
}

impl Player {
    /// Create a player for `window`.
    ///
    /// On X11/XWayland the native `wid` is discovered and libmpv is embedded.
    /// On pure Wayland no window id exists, so the external-process backend is
    /// selected instead.
    pub fn new(window: &gtk4::Window) -> Result<Self> {
        match x11_window_id(window) {
            Some(xid) => {
                let mpv = create_embedded_mpv(xid)?;
                Ok(Self {
                    backend: PlayerBackend::Libmpv,
                    mpv: Some(mpv),
                    external: None,
                })
            }
            None => Ok(Self {
                backend: PlayerBackend::External,
                mpv: None,
                external: None,
            }),
        }
    }

    pub fn backend(&self) -> PlayerBackend {
        self.backend
    }

    /// Load (and start playing) a stream URL.
    pub fn load(&mut self, url: &str) -> Result<()> {
        match self.backend {
            PlayerBackend::Libmpv => {
                let mpv = self
                    .mpv
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("libmpv backend without an mpv handle"))?;
                mpv.command("loadfile", &[url])
                    .map_err(|e| anyhow::anyhow!("mpv loadfile failed: {e}"))?;
                mpv.set_property("pause", false)
                    .map_err(|e| anyhow::anyhow!("mpv set pause failed: {e}"))?;
                Ok(())
            }
            PlayerBackend::External => {
                if let Some(mut child) = self.external.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let child = Command::new("mpv")
                    .arg("--force-window=yes")
                    .arg(url)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| anyhow::anyhow!("failed to launch external mpv: {e}"))?;
                self.external = Some(child);
                Ok(())
            }
        }
    }

    pub fn play(&self) -> Result<()> {
        if let Some(mpv) = &self.mpv {
            mpv.set_property("pause", false)
                .map_err(|e| anyhow::anyhow!("mpv set pause failed: {e}"))?;
        }
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        if let Some(mpv) = &self.mpv {
            mpv.set_property("pause", true)
                .map_err(|e| anyhow::anyhow!("mpv set pause failed: {e}"))?;
        }
        Ok(())
    }

    /// Toggle play/pause and return the resulting paused state (`true` = paused).
    pub fn toggle_pause(&self) -> Result<bool> {
        match &self.mpv {
            Some(mpv) => {
                let paused: bool = mpv.get_property("pause").unwrap_or(false);
                mpv.set_property("pause", !paused)
                    .map_err(|e| anyhow::anyhow!("mpv set pause failed: {e}"))?;
                Ok(!paused)
            }
            None => Ok(false),
        }
    }

    /// Seek by `seconds` relative to the current position.
    pub fn seek_relative(&self, seconds: f64) -> Result<()> {
        if let Some(mpv) = &self.mpv {
            let amount = seconds.to_string();
            mpv.command("seek", &[&amount, "relative"])
                .map_err(|e| anyhow::anyhow!("mpv seek failed: {e}"))?;
        }
        Ok(())
    }

    /// Seek to an absolute position in seconds.
    pub fn seek_absolute(&self, seconds: f64) -> Result<()> {
        if let Some(mpv) = &self.mpv {
            let amount = seconds.to_string();
            mpv.command("seek", &[&amount, "absolute"])
                .map_err(|e| anyhow::anyhow!("mpv seek failed: {e}"))?;
        }
        Ok(())
    }

    /// Set the volume on a 0..=100 scale.
    pub fn set_volume(&self, volume: f64) -> Result<()> {
        if let Some(mpv) = &self.mpv {
            let volume = volume.clamp(0.0, 100.0);
            mpv.set_property("volume", volume)
                .map_err(|e| anyhow::anyhow!("mpv set volume failed: {e}"))?;
        }
        Ok(())
    }

    /// Current playback position in seconds (`0.0` when unknown).
    pub fn position(&self) -> f64 {
        self.mpv
            .as_ref()
            .and_then(|mpv| mpv.get_property::<f64>("time-pos").ok())
            .unwrap_or(0.0)
    }

    /// Total duration in seconds (`0.0` when unknown).
    pub fn duration(&self) -> f64 {
        self.mpv
            .as_ref()
            .and_then(|mpv| mpv.get_property::<f64>("duration").ok())
            .unwrap_or(0.0)
    }

    /// Whether the underlying media is paused.
    pub fn is_paused(&self) -> bool {
        self.mpv
            .as_ref()
            .and_then(|mpv| mpv.get_property::<bool>("pause").ok())
            .unwrap_or(false)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(mut child) = self.external.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Extract the X11 window id behind a GTK window, if the window is realised on
/// an X11 (or XWayland) surface.
fn x11_window_id(window: &gtk4::Window) -> Option<u64> {
    let surface = window.surface()?;
    let x11 = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11.xid())
}

fn create_embedded_mpv(xid: u64) -> Result<Mpv> {
    Mpv::with_initializer(|init| {
        init.set_option("wid", xid as i64)?;
        init.set_option("vo", "gpu")?;
        init.set_option("force-window", "no")?;
        init.set_option("keep-open", "yes")?;
        init.set_option("idle", true)?;
        init.set_option("hwdec", "auto-safe")?;
        Ok(())
    })
    .map_err(|e| anyhow::anyhow!("failed to initialize libmpv: {e}"))
}
