//! The playback page: a video surface plus transport controls.
//!
//! See the module docs in `player.rs` for how libmpv is embedded. On X11 the
//! video covers the whole window, so the controls are primarily driven from
//! the keyboard; on the Wayland fallback the page shows a status message and
//! the controls operate the external process indirectly.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use flex_protocol::PlaybackMode;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Scale};
use libadwaita as adw;

use super::format_duration_secs;
use crate::player::{Player, PlayerBackend};

/// Human-readable label for a playback mode.
pub fn mode_label(mode: PlaybackMode) -> &'static str {
    match mode {
        PlaybackMode::Direct => "Direct play",
        PlaybackMode::Remux => "Remux",
        PlaybackMode::Transcode => "Transcode",
    }
}

/// State and widgets for the player page.
pub struct PlayerPage {
    pub page: adw::NavigationPage,
    player: Rc<RefCell<Option<Player>>>,
    seek: Scale,
    position_label: Label,
    duration_label: Label,
    play_button: Button,
    status_label: Label,
    updating: Rc<Cell<bool>>,
}

impl PlayerPage {
    /// Build the page. `player` is shared with the rest of the app and filled
    /// in on the first playback request.
    pub fn new(player: Rc<RefCell<Option<Player>>>, window: &gtk4::Window) -> Self {
        let updating = Rc::new(Cell::new(false));

        let surface_label = Label::new(Some("Video surface"));
        surface_label.add_css_class("dim-label");
        surface_label.set_valign(Align::Center);
        let surface = GtkBox::new(Orientation::Vertical, 0);
        surface.set_vexpand(true);
        surface.add_css_class("view");
        surface.append(&surface_label);

        // Seek row.
        let seek = Scale::with_range(Orientation::Horizontal, 0.0, 1.0, 1.0);
        seek.set_draw_value(false);
        seek.set_hexpand(true);
        seek.set_valign(Align::Center);

        let position_label = Label::new(Some("--:--"));
        position_label.add_css_class("numeric");
        let duration_label = Label::new(Some("--:--"));
        duration_label.add_css_class("numeric");

        let seek_row = GtkBox::new(Orientation::Horizontal, 8);
        seek_row.append(&position_label);
        seek_row.append(&seek);
        seek_row.append(&duration_label);

        // Transport row.
        let play_button = Button::from_icon_name("media-playback-start-symbolic");
        play_button.add_css_class("circular");
        play_button.add_css_class("suggested-action");
        play_button.set_tooltip_text(Some("Play/Pause (Space)"));

        let back_button = Button::from_icon_name("go-previous-symbolic");
        back_button.add_css_class("circular");
        back_button.set_tooltip_text(Some("Back 5s (Left)"));
        let forward_button = Button::from_icon_name("go-next-symbolic");
        forward_button.add_css_class("circular");
        forward_button.set_tooltip_text(Some("Forward 5s (Right)"));

        let volume_icon = gtk4::Image::from_icon_name("audio-volume-high-symbolic");
        let volume = Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
        volume.set_draw_value(false);
        volume.set_value(100.0);
        volume.set_size_request(120, -1);
        volume.set_valign(Align::Center);

        let fullscreen_button = Button::from_icon_name("view-fullscreen-symbolic");
        fullscreen_button.add_css_class("circular");
        fullscreen_button.set_tooltip_text(Some("Fullscreen (f)"));

        let transport = GtkBox::new(Orientation::Horizontal, 8);
        transport.set_halign(Align::Center);
        transport.append(&back_button);
        transport.append(&play_button);
        transport.append(&forward_button);

        let right_cluster = GtkBox::new(Orientation::Horizontal, 8);
        right_cluster.set_halign(Align::End);
        right_cluster.set_hexpand(true);
        right_cluster.append(&volume_icon);
        right_cluster.append(&volume);
        right_cluster.append(&fullscreen_button);

        let transport_row = GtkBox::new(Orientation::Horizontal, 8);
        transport_row.append(&transport);
        transport_row.append(&right_cluster);

        let status_label = Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.set_visible(false);

        let controls = GtkBox::new(Orientation::Vertical, 6);
        controls.set_margin_start(12);
        controls.set_margin_end(12);
        controls.set_margin_top(8);
        controls.set_margin_bottom(12);
        controls.append(&seek_row);
        controls.append(&transport_row);
        controls.append(&status_label);

        let root = GtkBox::new(Orientation::Vertical, 0);
        root.append(&surface);
        root.append(&controls);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&root));

        let page = adw::NavigationPage::new(&toolbar, "Playing");

        // --- signal wiring (all widgets still in scope here) ---
        play_button.connect_clicked({
            let player = player.clone();
            move |_| {
                if let Some(player) = player.borrow().as_ref() {
                    let _ = player.toggle_pause();
                }
            }
        });
        back_button.connect_clicked({
            let player = player.clone();
            move |_| {
                if let Some(player) = player.borrow().as_ref() {
                    let _ = player.seek_relative(-5.0);
                }
            }
        });
        forward_button.connect_clicked({
            let player = player.clone();
            move |_| {
                if let Some(player) = player.borrow().as_ref() {
                    let _ = player.seek_relative(5.0);
                }
            }
        });
        volume.connect_value_changed({
            let player = player.clone();
            move |scale| {
                if let Some(player) = player.borrow().as_ref() {
                    let _ = player.set_volume(scale.value());
                }
            }
        });
        seek.connect_value_changed({
            let player = player.clone();
            let updating = updating.clone();
            move |scale| {
                if updating.get() {
                    return;
                }
                if let Some(player) = player.borrow().as_ref() {
                    let _ = player.seek_absolute(scale.value());
                }
            }
        });
        fullscreen_button.connect_clicked({
            let window = window.clone();
            move |_| toggle_fullscreen(&window)
        });

        Self {
            page,
            player,
            seek,
            position_label,
            duration_label,
            play_button,
            status_label,
            updating,
        }
    }

    /// Show a short status line describing the active playback mode and backend.
    pub fn set_status(&self, backend: PlayerBackend, mode: Option<PlaybackMode>) {
        let backend_text = match backend {
            PlayerBackend::Libmpv => "Embedded playback via libmpv",
            PlayerBackend::External => {
                "External mpv window (Wayland fallback) — use mpv's own window for controls"
            }
        };
        let text = match mode {
            Some(mode) => format!("{} · {}", mode_label(mode), backend_text),
            None => backend_text.to_string(),
        };
        self.status_label.set_text(&text);
        self.status_label.set_visible(true);
    }

    /// Begin polling the player every 250 ms and updating the controls.
    pub fn start_polling(&self) {
        let player = self.player.clone();
        let seek = self.seek.clone();
        let position_label = self.position_label.clone();
        let duration_label = self.duration_label.clone();
        let play_button = self.play_button.clone();
        let updating = self.updating.clone();

        glib::timeout_add_local(Duration::from_millis(250), move || {
            if let Some(player) = player.borrow().as_ref() {
                let duration = player.duration();
                let position = player.position();

                updating.set(true);
                seek.set_range(0.0, duration.max(1.0));
                seek.set_value(position.clamp(0.0, duration.max(1.0)));
                updating.set(false);

                position_label.set_text(&format_duration_secs(position));
                duration_label.set_text(&format_duration_secs(duration));
                play_button.set_icon_name(if player.is_paused() {
                    "media-playback-start-symbolic"
                } else {
                    "media-playback-pause-symbolic"
                });
            }
            glib::ControlFlow::Continue
        });
    }
}

/// Toggle fullscreen on `window`.
pub fn toggle_fullscreen(window: &gtk4::Window) {
    if window.is_fullscreen() {
        window.unfullscreen();
    } else {
        window.fullscreen();
    }
}
