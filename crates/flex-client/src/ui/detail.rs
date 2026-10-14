//! The media item detail page.

use flex_protocol::MediaItem;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation};
use libadwaita as adw;

use super::format_duration_ms;

/// Build a scrollable detail page for `item`. `on_play` is invoked when the
/// prominent Play button is pressed. When `resume_ms` is set the button offers
/// to resume from that position (the server resumes automatically).
pub fn build_detail_page(
    item: &MediaItem,
    resume_ms: Option<u64>,
    on_play: impl Fn() + 'static,
) -> adw::NavigationPage {
    let content = GtkBox::new(Orientation::Vertical, 14);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(18);
    content.set_margin_bottom(18);

    let title = Label::new(Some(&item.title));
    title.set_wrap(true);
    title.set_xalign(0.0);
    title.add_css_class("title-1");

    content.append(&title);

    let meta = GtkBox::new(Orientation::Horizontal, 8);
    let year = Label::new(item.year.map(|y| y.to_string()).as_deref());
    year.add_css_class("dim-label");
    meta.append(&year);
    let dot = Label::new(Some("•"));
    dot.add_css_class("dim-label");
    meta.append(&dot);
    let duration = Label::new(Some(&format_duration_ms(item.duration_ms)));
    duration.add_css_class("dim-label");
    meta.append(&duration);
    content.append(&meta);

    if let Some(video) = &item.video {
        let mut line = format!("{} · {}×{}", video.codec, video.width, video.height);
        if let Some(fps) = video.fps {
            line.push_str(&format!(" · {fps:.2} fps"));
        }
        let video_label = Label::new(Some(&line));
        video_label.set_xalign(0.0);
        video_label.add_css_class("dim-label");
        content.append(&video_label);
    }

    let play_label = match resume_ms {
        Some(ms) if ms > 0 => format!("Resume from {}", super::format_duration_ms(Some(ms))),
        _ => "Play".to_string(),
    };
    let play_button = Button::with_label(&play_label);
    play_button.add_css_class("suggested-action");
    play_button.add_css_class("pill");
    play_button.set_halign(Align::Start);
    play_button.set_margin_top(4);
    play_button.set_margin_bottom(4);
    play_button.connect_clicked(move |_| on_play());
    content.append(&play_button);

    if let Some(overview) = item.overview.as_deref().filter(|o| !o.is_empty()) {
        let overview_label = Label::new(Some(overview));
        overview_label.set_wrap(true);
        overview_label.set_xalign(0.0);
        content.append(&overview_label);
    }

    if !item.audio.is_empty() {
        content.append(&section_heading("Audio tracks"));
        for audio in &item.audio {
            let mut parts = vec![format!("#{} {}", audio.index, audio.codec)];
            if let Some(channels) = audio.channels {
                parts.push(format!("{channels} ch"));
            }
            if let Some(language) = &audio.language {
                parts.push(language.clone());
            }
            if let Some(title) = &audio.title {
                parts.push(title.clone());
            }
            content.append(&track_label(&parts.join(" · ")));
        }
    }

    if !item.subtitles.is_empty() {
        content.append(&section_heading("Subtitles"));
        for subtitle in &item.subtitles {
            let mut parts = vec![format!("#{} {}", subtitle.index, subtitle.codec)];
            if let Some(language) = &subtitle.language {
                parts.push(language.clone());
            }
            if let Some(title) = &subtitle.title {
                parts.push(title.clone());
            }
            if subtitle.forced {
                parts.push("forced".into());
            }
            if subtitle.default {
                parts.push("default".into());
            }
            content.append(&track_label(&parts.join(" · ")));
        }
    }

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(760);
    clamp.set_tightening_threshold(600);
    clamp.set_child(Some(&content));

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_child(Some(&clamp));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scrolled));

    adw::NavigationPage::new(&toolbar, &item.title)
}

fn section_heading(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("heading");
    label.set_margin_top(8);
    label
}

fn track_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.add_css_class("dim-label");
    label
}
