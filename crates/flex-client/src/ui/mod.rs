//! GTK widgets that make up the Flex browser: poster cards, the item detail
//! page, and the playback page.

mod card;
mod detail;
mod login;
pub mod player_page;
mod series;

pub use card::{build_card, build_continue_card, build_series_card, Card};
pub use detail::build_detail_page;
pub use login::LoginPage;
pub use player_page::PlayerPage;
pub use series::build_series_page;

/// Format a duration given in milliseconds as `mm:ss`, or `h:mm:ss` when the
/// value is an hour or longer. `None` renders as `--:--`.
pub fn format_duration_ms(duration_ms: Option<u64>) -> String {
    match duration_ms {
        Some(ms) => format_duration_secs(ms as f64 / 1000.0),
        None => "--:--".to_string(),
    }
}

/// Format a duration given in (possibly fractional) seconds.
pub fn format_duration_secs(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".to_string();
    }
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
