//! A single poster card shown inside the library grid.

use flex_protocol::{MediaItem, Series};
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Image, Label, Orientation, Picture, ProgressBar, Stack};

/// Widgets backing one card. The artwork is filled in asynchronously by the
/// caller once the image bytes have been fetched.
pub struct Card {
    pub button: Button,
    pub picture: Picture,
    pub stack: Stack,
    pub progress: ProgressBar,
}

/// Build a card for `item`. The artwork stack starts on a symbolic placeholder
/// and is switched to the real poster when [`Card::picture`] is populated.
pub fn build_card(item: &MediaItem) -> Card {
    let subtitle = item.year.map(|year| year.to_string()).unwrap_or_default();
    let card = build_card_inner(&item.title, &subtitle, "video-x-generic-symbolic");
    card.progress.set_visible(false);
    card
}

/// Build a card that additionally shows a progress bar under the title.
pub fn build_continue_card(item: &MediaItem, fraction: f64) -> Card {
    let subtitle = item.year.map(|year| year.to_string()).unwrap_or_default();
    let card = build_card_inner(&item.title, &subtitle, "video-x-generic-symbolic");
    card.progress.set_visible(true);
    card.progress.set_fraction(fraction.clamp(0.0, 1.0));
    card
}

/// Build a card for a TV series, showing its year and season count.
pub fn build_series_card(series: &Series) -> Card {
    let seasons = match series.season_count {
        1 => "1 season".to_string(),
        count => format!("{count} seasons"),
    };
    let subtitle = match series.year {
        Some(year) => format!("{year} · {seasons}"),
        None => seasons,
    };
    let card = build_card_inner(&series.title, &subtitle, "video-display-symbolic");
    card.progress.set_visible(false);
    card
}

/// Build shared card widgets with a placeholder image, labels, and progress bar.
fn build_card_inner(title_text: &str, subtitle_text: &str, placeholder_icon: &str) -> Card {
    let picture = Picture::new();
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(150, 225);

    let placeholder = Image::from_icon_name(placeholder_icon);
    placeholder.set_pixel_size(64);
    placeholder.add_css_class("dim-label");

    let stack = Stack::new();
    stack.add_named(&picture, Some("art"));
    stack.add_named(&placeholder, Some("placeholder"));
    stack.set_visible_child_name("placeholder");
    stack.set_size_request(150, 225);

    let title = Label::new(Some(title_text));
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(18);
    title.set_halign(Align::Start);
    title.set_xalign(0.0);

    let subtitle = Label::new(Some(subtitle_text));
    subtitle.add_css_class("dim-label");
    subtitle.set_halign(Align::Start);
    subtitle.set_xalign(0.0);

    let progress = ProgressBar::new();
    progress.set_size_request(150, -1);
    progress.set_margin_top(2);

    let content = GtkBox::new(Orientation::Vertical, 4);
    content.set_margin_start(6);
    content.set_margin_end(6);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.append(&stack);
    content.append(&title);
    content.append(&subtitle);
    content.append(&progress);

    let button = Button::new();
    button.set_child(Some(&content));
    button.add_css_class("flat");
    button.add_css_class("card");
    button.set_tooltip_text(Some(title_text));

    Card {
        button,
        picture,
        stack,
        progress,
    }
}
