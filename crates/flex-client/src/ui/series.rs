//! The TV series page: seasons and their episodes.

use std::rc::Rc;

use flex_protocol::SeriesDetail;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation};
use libadwaita as adw;

use super::format_duration_ms;

/// Build a scrollable page for `detail` listing every season and episode.
/// `on_episode` is invoked with the underlying media item id when an episode is
/// activated.
pub fn build_series_page(
    detail: &SeriesDetail,
    on_episode: impl Fn(i64) + 'static,
) -> adw::NavigationPage {
    let series = &detail.series;
    let on_episode: Rc<dyn Fn(i64)> = Rc::new(on_episode);

    let content = GtkBox::new(Orientation::Vertical, 14);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(18);
    content.set_margin_bottom(18);

    let title = Label::new(Some(&series.title));
    title.set_wrap(true);
    title.set_xalign(0.0);
    title.add_css_class("title-1");
    content.append(&title);

    let seasons_text = match series.season_count {
        1 => "1 season".to_string(),
        count => format!("{count} seasons"),
    };
    let episodes_text = match series.episode_count {
        1 => "1 episode".to_string(),
        count => format!("{count} episodes"),
    };
    let meta = Label::new(Some(
        &[
            series.year.map(|year| year.to_string()),
            Some(seasons_text),
            Some(episodes_text),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
    ));
    meta.add_css_class("dim-label");
    meta.set_xalign(0.0);
    content.append(&meta);

    if let Some(overview) = series.overview.as_deref().filter(|o| !o.is_empty()) {
        let overview_label = Label::new(Some(overview));
        overview_label.set_wrap(true);
        overview_label.set_xalign(0.0);
        content.append(&overview_label);
    }

    for season in &detail.seasons {
        let heading = Label::new(Some(&if season.season_number > 0 {
            format!("Season {}", season.season_number)
        } else {
            "Specials".to_string()
        }));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        heading.set_margin_top(12);
        content.append(&heading);

        let list = GtkBox::new(Orientation::Vertical, 2);
        for episode in &season.episodes {
            let number = match episode.episode {
                Some(n) if n > 0 => format!("E{n:02}"),
                _ => "—".to_string(),
            };
            let duration = format_duration_ms(episode.duration_ms);
            let label = format!("{number}  ·  {}  ·  {duration}", episode.title);
            let row = Button::with_label(&label);
            row.set_halign(Align::Fill);
            row.add_css_class("flat");
            let episode_id = episode.id;
            let on_episode = on_episode.clone();
            row.connect_clicked(move |_| on_episode(episode_id));
            list.append(&row);
        }
        content.append(&list);
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

    adw::NavigationPage::new(&toolbar, &series.title)
}
