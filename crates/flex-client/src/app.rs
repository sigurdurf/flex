//! Window construction and UI wiring.
//!
//! The whole browser is built in code (no `.gresource`/blueprint files). A
//! top-level [`gtk4::Stack`] swaps between the login page and the main
//! [`adw::NavigationSplitView`] whose sidebar lists the libraries and whose
//! content hosts a [`adw::NavigationView`] onto which detail and player pages
//! are pushed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use flex_protocol::{
    ContinueWatchingEntry, ItemQuery, Library, LibraryKind, MediaItem, PlaybackSession,
    PlaybackStartRequest, Series, SeriesQuery, UpdateProgressRequest,
};
use gtk4::prelude::*;
use gtk4::{gdk, glib, Align, Box as GtkBox, Button, Orientation, SelectionMode};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::api::{ApiClient, ApiError, ApiErrorKind};
use crate::player::{client_capabilities, Player};
use crate::session;
use crate::ui::{
    build_card, build_continue_card, build_detail_page, build_series_card, build_series_page,
    LoginPage, PlayerPage,
};

/// The currently running playback session, if any.
struct ActivePlayback {
    session_id: String,
    item_id: i64,
}

/// Shared application state. Widgets are reference-counted GObjects, so cloning
/// the handles into signal closures is cheap.
pub struct AppState {
    api: ApiClient,
    window: gtk4::Window,
    root_stack: gtk4::Stack,
    login_page: LoginPage,
    nav_view: adw::NavigationView,
    flow: gtk4::FlowBox,
    continue_box: GtkBox,
    continue_section: GtkBox,
    browse_stack: gtk4::Stack,
    status_page: adw::StatusPage,
    sidebar_list: gtk4::ListBox,
    search: gtk4::SearchEntry,
    toast_overlay: adw::ToastOverlay,
    library_ids: RefCell<Vec<Option<i64>>>,
    library_kinds: RefCell<Vec<Option<LibraryKind>>>,
    current_library: Cell<Option<i64>>,
    current_kind: Cell<Option<LibraryKind>>,
    player: Rc<RefCell<Option<Player>>>,
    player_page: Rc<PlayerPage>,
    active: RefCell<Option<ActivePlayback>>,
    last_progress: RefCell<Option<Instant>>,
}

/// Build the application window and kick off the initial data loads.
pub fn build_ui(app: &adw::Application, api: ApiClient) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Flex")
        .default_width(1100)
        .default_height(720)
        .build();

    let window_widget: gtk4::Window = window.clone().upcast();

    let player: Rc<RefCell<Option<Player>>> = Rc::new(RefCell::new(None));
    let player_page = Rc::new(PlayerPage::new(player.clone(), &window_widget));

    let login_page = LoginPage::new(&api.base_url());

    // --- Sidebar ---------------------------------------------------------
    let search = gtk4::SearchEntry::new();
    search.set_placeholder_text(Some("Search…"));
    search.set_margin_start(6);
    search.set_margin_end(6);
    search.set_margin_top(6);
    search.set_margin_bottom(6);

    let scan_button = Button::from_icon_name("view-refresh-symbolic");
    scan_button.set_tooltip_text(Some("Scan library"));
    scan_button.add_css_class("flat");

    let sidebar_list = gtk4::ListBox::new();
    sidebar_list.set_selection_mode(SelectionMode::Single);
    sidebar_list.add_css_class("navigation-sidebar");

    let sidebar_scroll = gtk4::ScrolledWindow::new();
    sidebar_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    sidebar_scroll.set_vexpand(true);
    sidebar_scroll.set_child(Some(&sidebar_list));

    let sidebar_content = GtkBox::new(Orientation::Vertical, 0);
    sidebar_content.append(&search);
    sidebar_content.append(&sidebar_scroll);

    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.pack_end(&scan_button);
    let sidebar_title = adw::WindowTitle::new("Flex", "Media browser");
    sidebar_header.set_title_widget(Some(&sidebar_title));

    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(&sidebar_content));

    let sidebar_page = adw::NavigationPage::new(&sidebar_toolbar, "Flex");

    // --- Continue watching ----------------------------------------------
    let continue_box = GtkBox::new(Orientation::Horizontal, 12);
    continue_box.set_margin_start(12);
    continue_box.set_margin_end(12);
    continue_box.set_margin_top(6);
    continue_box.set_margin_bottom(6);

    let continue_scroll = gtk4::ScrolledWindow::new();
    continue_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
    continue_scroll.set_child(Some(&continue_box));

    let continue_heading = gtk4::Label::new(Some("Continue watching"));
    continue_heading.set_xalign(0.0);
    continue_heading.add_css_class("heading");
    continue_heading.set_margin_start(12);
    continue_heading.set_margin_top(8);

    let continue_section = GtkBox::new(Orientation::Vertical, 4);
    continue_section.append(&continue_heading);
    continue_section.append(&continue_scroll);
    continue_section.set_visible(false);

    // --- Browse content --------------------------------------------------
    let flow = gtk4::FlowBox::new();
    flow.set_selection_mode(SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_row_spacing(12);
    flow.set_column_spacing(12);
    flow.set_margin_start(12);
    flow.set_margin_end(12);
    flow.set_margin_top(12);
    flow.set_margin_bottom(12);
    flow.set_valign(Align::Start);
    flow.set_halign(Align::Fill);

    let flow_scroll = gtk4::ScrolledWindow::new();
    flow_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    flow_scroll.set_vexpand(true);
    flow_scroll.set_child(Some(&flow));

    let browse_content = GtkBox::new(Orientation::Vertical, 0);
    browse_content.append(&continue_section);
    browse_content.append(&flow_scroll);

    let status_page = adw::StatusPage::new();
    status_page.set_vexpand(true);
    status_page.set_icon_name(Some("folder-videos-symbolic"));
    status_page.set_title("Loading…");

    let browse_stack = gtk4::Stack::new();
    browse_stack.add_named(&browse_content, Some("grid"));
    browse_stack.add_named(&status_page, Some("status"));
    browse_stack.set_visible_child_name("status");

    let browse_header = adw::HeaderBar::new();
    let browse_title = adw::WindowTitle::new("Browse", &api.base_url());
    browse_header.set_title_widget(Some(&browse_title));

    let logout_button = Button::from_icon_name("application-exit-symbolic");
    logout_button.set_tooltip_text(Some("Log out"));
    logout_button.add_css_class("flat");
    browse_header.pack_end(&logout_button);

    let browse_toolbar = adw::ToolbarView::new();
    browse_toolbar.add_top_bar(&browse_header);
    browse_toolbar.set_content(Some(&browse_stack));

    let browse_page = adw::NavigationPage::new(&browse_toolbar, "Browse");

    let nav_view = adw::NavigationView::new();
    nav_view.add(&browse_page);

    let content_page = adw::NavigationPage::new(&nav_view, "Browse");

    // --- Assemble --------------------------------------------------------
    let split_view = adw::NavigationSplitView::new();
    split_view.set_sidebar(Some(&sidebar_page));
    split_view.set_content(Some(&content_page));
    split_view.set_sidebar_width_fraction(0.26);

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&split_view));

    let root_stack = gtk4::Stack::new();
    root_stack.add_named(&login_page.widget, Some("login"));
    root_stack.add_named(&toast_overlay, Some("main"));
    root_stack.set_visible_child_name("login");
    window.set_content(Some(&root_stack));

    let state = Rc::new(AppState {
        api,
        window: window_widget,
        root_stack,
        login_page,
        nav_view,
        flow,
        continue_box,
        continue_section,
        browse_stack,
        status_page,
        sidebar_list: sidebar_list.clone(),
        search: search.clone(),
        toast_overlay: toast_overlay.clone(),
        library_ids: RefCell::new(Vec::new()),
        library_kinds: RefCell::new(Vec::new()),
        current_library: Cell::new(None),
        current_kind: Cell::new(None),
        player: player.clone(),
        player_page: player_page.clone(),
        active: RefCell::new(None),
        last_progress: RefCell::new(None),
    });

    connect_signals(&state, &scan_button, &logout_button);
    attach_keyboard_shortcuts(&state);

    // Stop and save progress when the player page is dismissed.
    state.player_page.page.connect_unmap({
        let state = state.clone();
        move |_| stop_playback(&state, true)
    });

    window.connect_close_request({
        let state = state.clone();
        move |_| {
            stop_playback(&state, true);
            glib::Propagation::Proceed
        }
    });

    state.player_page.start_polling();
    start_progress_timer(&state);

    if state.api.token().is_some() {
        validate_session(&state);
    } else {
        show_login(&state, None);
    }

    window
}

/// Validate a stored token via `GET /auth/me`, then enter the main UI.
fn validate_session(state: &Rc<AppState>) {
    state.login_page.set_busy(true);
    state.login_page.set_status("Restoring session…");
    let api = state.api.clone();
    state.api.dispatch(async move { api.me().await }, {
        let state = state.clone();
        move |result| {
            state.login_page.set_busy(false);
            match result {
                Ok(_) => enter_main(&state),
                Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                Err(err) => show_login(&state, Some(&format!("Could not reach server: {err:#}"))),
            }
        }
    });
}

/// Switch to the main browser view and refresh all data.
fn enter_main(state: &Rc<AppState>) {
    state.login_page.set_status("");
    state.root_stack.set_visible_child_name("main");
    check_health(state);
    load_libraries(state);
    refresh_items(state);
    load_continue_watching(state);
}

fn show_login(state: &Rc<AppState>, status: Option<&str>) {
    state.login_page.set_busy(false);
    state.login_page.set_status(status.unwrap_or(""));
    state.root_stack.set_visible_child_name("login");
}

/// Clear local credentials and return to the login page after a `401`.
fn bounce_to_login(state: &Rc<AppState>) {
    state.api.clear_token();
    let _ = session::clear();
    stop_playback(state, false);
    show_login(state, Some("Session expired — please sign in again."));
}

/// Whether an error originated from a `401 Unauthorized` response.
fn is_unauthorized(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ApiError>()
        .is_some_and(|e| e.kind == ApiErrorKind::Unauthorized)
}

fn check_health(state: &Rc<AppState>) {
    let api = state.api.clone();
    state.api.dispatch(async move { api.health().await }, {
        let state = state.clone();
        move |result| match result {
            Ok(health) => toast(
                &state,
                &format!(
                    "Connected to Flex {} (API {})",
                    health.version, health.api_version
                ),
            ),
            Err(err) => toast(&state, &format!("Server unreachable: {err:#}")),
        }
    });
}

fn connect_signals(state: &Rc<AppState>, scan_button: &Button, logout_button: &Button) {
    scan_button.connect_clicked({
        let state = state.clone();
        move |_| {
            let api = state.api.clone();
            state.api.dispatch(async move { api.scan().await }, {
                let state = state.clone();
                move |result| match result {
                    Ok(report) => {
                        toast(
                            &state,
                            &format!(
                                "Scan complete: {} scanned, {} added, {} updated",
                                report.scanned, report.added, report.updated
                            ),
                        );
                        load_libraries(&state);
                        refresh_items(&state);
                    }
                    Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                    Err(err) => toast(&state, &format!("Scan failed: {err:#}")),
                }
            });
        }
    });

    logout_button.connect_clicked({
        let state = state.clone();
        move |_| logout(&state)
    });

    state.login_page.login_button.connect_clicked({
        let state = state.clone();
        move |_| submit_login(&state)
    });

    state.sidebar_list.connect_row_activated({
        let state = state.clone();
        move |_, row| {
            let index = row.index().max(0) as usize;
            let id = state.library_ids.borrow().get(index).copied().flatten();
            let kind = state.library_kinds.borrow().get(index).copied().flatten();
            state.current_library.set(id);
            state.current_kind.set(kind);
            refresh_items(&state);
        }
    });

    state.search.connect_search_changed({
        let state = state.clone();
        move |_| refresh_items(&state)
    });
}

fn submit_login(state: &Rc<AppState>) {
    let server = state.login_page.server_entry.text().to_string();
    let username = state.login_page.username_entry.text().to_string();
    let password = state.login_page.password_entry.text().to_string();
    let device = state.login_page.device_entry.text().to_string();

    if server.trim().is_empty() || username.trim().is_empty() {
        state
            .login_page
            .set_status("Server URL and username are required.");
        return;
    }

    state.login_page.set_busy(true);
    state.login_page.set_status("");
    let api = state.api.clone();
    let device = (!device.trim().is_empty()).then_some(device);
    state.api.dispatch(
        async move {
            api.set_base(server);
            api.login(&username, &password, device.as_deref()).await
        },
        {
            let state = state.clone();
            move |result| {
                state.login_page.set_busy(false);
                match result {
                    Ok(response) => {
                        let stored = session::Session {
                            server_url: state.api.base_url(),
                            token: response.token.clone(),
                            username: response.user.username.clone(),
                        };
                        if let Err(err) = session::save(&stored) {
                            toast(&state, &format!("Could not save session: {err:#}"));
                        }
                        state.api.set_token(Some(response.token));
                        enter_main(&state);
                    }
                    Err(err) => state
                        .login_page
                        .set_status(&format!("Sign-in failed: {err:#}")),
                }
            }
        },
    );
}

fn logout(state: &Rc<AppState>) {
    stop_playback(state, false);
    let api = state.api.clone();
    state.api.dispatch(async move { api.logout().await }, {
        let state = state.clone();
        move |_| {
            state.api.clear_token();
            let _ = session::clear();
            show_login(&state, None);
        }
    });
}

fn attach_keyboard_shortcuts(state: &Rc<AppState>) {
    let controller = gtk4::EventControllerKey::new();
    controller.connect_key_pressed({
        let state = state.clone();
        move |_, key, _, _| {
            let slot = state.player.borrow();
            let Some(player) = slot.as_ref() else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::space {
                let _ = player.toggle_pause();
                glib::Propagation::Stop
            } else if key == gdk::Key::Left {
                let _ = player.seek_relative(-5.0);
                glib::Propagation::Stop
            } else if key == gdk::Key::Right {
                let _ = player.seek_relative(5.0);
                glib::Propagation::Stop
            } else {
                drop(slot);
                if key == gdk::Key::f {
                    crate::ui::player_page::toggle_fullscreen(&state.window);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        }
    });
    state.window.add_controller(controller);
}

fn load_libraries(state: &Rc<AppState>) {
    let api = state.api.clone();
    state.api.dispatch(async move { api.libraries().await }, {
        let state = state.clone();
        move |result| match result {
            Ok(libraries) => populate_libraries(&state, &libraries),
            Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
            Err(err) => {
                show_status(
                    &state,
                    "folder-videos-symbolic",
                    "Cannot reach server",
                    &format!("{err:#}"),
                );
                toast(&state, &format!("Failed to load libraries: {err:#}"));
            }
        }
    });
}

fn populate_libraries(state: &Rc<AppState>, libraries: &[Library]) {
    while let Some(child) = state.sidebar_list.first_child() {
        child.unparent();
    }
    let mut ids: Vec<Option<i64>> = Vec::with_capacity(libraries.len() + 1);
    let mut kinds: Vec<Option<LibraryKind>> = Vec::with_capacity(libraries.len() + 1);
    ids.push(None);
    kinds.push(None);

    let all_row = adw::ActionRow::builder().title("All").build();
    all_row.add_prefix(&gtk4::Image::from_icon_name("view-grid-symbolic"));
    state.sidebar_list.append(&all_row);

    for library in libraries {
        ids.push(Some(library.id));
        kinds.push(Some(library.kind));
        let row = adw::ActionRow::builder()
            .title(library.name.clone())
            .subtitle(format!("{} items", library.item_count))
            .build();
        row.add_prefix(&gtk4::Image::from_icon_name(&library_icon(library)));
        state.sidebar_list.append(&row);
    }

    *state.library_ids.borrow_mut() = ids;
    *state.library_kinds.borrow_mut() = kinds;
    if let Some(first) = state.sidebar_list.row_at_index(0) {
        state.sidebar_list.select_row(Some(&first));
    }
}

fn library_icon(library: &Library) -> String {
    match library.kind {
        flex_protocol::LibraryKind::Movies => "video-x-generic-symbolic",
        flex_protocol::LibraryKind::Tv => "video-display-symbolic",
        flex_protocol::LibraryKind::Music => "audio-x-generic-symbolic",
        flex_protocol::LibraryKind::Other => "folder-symbolic",
    }
    .to_string()
}

fn refresh_items(state: &Rc<AppState>) {
    if state.current_kind.get() == Some(LibraryKind::Tv) {
        refresh_series(state);
        return;
    }

    let query = ItemQuery {
        library_id: state.current_library.get(),
        kind: None,
        q: {
            let text = state.search.text().to_string();
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        },
        sort: Some("title".to_string()),
        limit: Some(500),
        offset: None,
    };

    state.browse_stack.set_visible_child_name("status");
    state
        .status_page
        .set_icon_name(Some("content-loading-symbolic"));
    state.status_page.set_title("Loading…");
    state.status_page.set_description(None);

    let api = state.api.clone();
    state.api.dispatch(async move { api.items(&query).await }, {
        let state = state.clone();
        move |result| match result {
            Ok(page) => populate_flow(&state, page.items),
            Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
            Err(err) => {
                show_status(
                    &state,
                    "network-error-symbolic",
                    "Failed to load items",
                    &format!("{err:#}"),
                );
            }
        }
    });
}

fn populate_flow(state: &Rc<AppState>, items: Vec<MediaItem>) {
    while let Some(child) = state.flow.first_child() {
        child.unparent();
    }
    if items.is_empty() {
        show_status(
            state,
            "folder-videos-symbolic",
            "No media found",
            "Scan a library or adjust your search.",
        );
        return;
    }

    for item in items {
        let card = build_card(&item);
        state.flow.append(&card.button);

        let id = item.id;
        let state_click = state.clone();
        card.button
            .connect_clicked(move |_| open_detail(&state_click, id));
        fill_artwork(state, id, card);
    }

    state.browse_stack.set_visible_child_name("grid");
}

fn refresh_series(state: &Rc<AppState>) {
    let query = SeriesQuery {
        library_id: state.current_library.get(),
        q: {
            let text = state.search.text().to_string();
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        },
        sort: Some("title".to_string()),
        limit: Some(500),
        offset: None,
    };

    state.browse_stack.set_visible_child_name("status");
    state
        .status_page
        .set_icon_name(Some("content-loading-symbolic"));
    state.status_page.set_title("Loading…");
    state.status_page.set_description(None);

    let api = state.api.clone();
    state
        .api
        .dispatch(async move { api.series(&query).await }, {
            let state = state.clone();
            move |result| match result {
                Ok(page) => populate_series_flow(&state, page.items),
                Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                Err(err) => {
                    show_status(
                        &state,
                        "network-error-symbolic",
                        "Failed to load series",
                        &format!("{err:#}"),
                    );
                }
            }
        });
}

fn populate_series_flow(state: &Rc<AppState>, series: Vec<Series>) {
    while let Some(child) = state.flow.first_child() {
        child.unparent();
    }
    if series.is_empty() {
        show_status(
            state,
            "video-display-symbolic",
            "No series found",
            "Scan a library or adjust your search.",
        );
        return;
    }

    for show in series {
        let card = build_series_card(&show);
        state.flow.append(&card.button);

        let id = show.id;
        let state_click = state.clone();
        card.button
            .connect_clicked(move |_| open_series(&state_click, id));
        fill_series_artwork(state, id, card);
    }

    state.browse_stack.set_visible_child_name("grid");
}

fn fill_series_artwork(state: &Rc<AppState>, id: i64, card: crate::ui::Card) {
    let api = state.api.clone();
    let card = Rc::new(card);
    state.api.dispatch(
        async move { api.series_artwork_bytes(id, "poster").await },
        move |result| {
            if let Ok(Some(bytes)) = result {
                let bytes = glib::Bytes::from(&bytes);
                if let Ok(texture) = gdk::Texture::from_bytes(&bytes) {
                    card.picture.set_paintable(Some(&texture));
                    card.stack.set_visible_child_name("art");
                }
            }
        },
    );
}

fn open_series(state: &Rc<AppState>, id: i64) {
    let api = state.api.clone();
    state
        .api
        .dispatch(async move { api.series_detail(id).await }, {
            let state = state.clone();
            move |result| match result {
                Ok(detail) => {
                    let state_episode = state.clone();
                    let page = build_series_page(&detail, move |episode_id| {
                        open_detail(&state_episode, episode_id);
                    });
                    state.nav_view.push(&page);
                }
                Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                Err(err) => toast(&state, &format!("Failed to load series: {err:#}")),
            }
        });
}

fn load_continue_watching(state: &Rc<AppState>) {
    let api = state.api.clone();
    state
        .api
        .dispatch(async move { api.continue_watching().await }, {
            let state = state.clone();
            move |result| match result {
                Ok(entries) => populate_continue(&state, entries),
                Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                Err(_) => state.continue_section.set_visible(false),
            }
        });
}

fn populate_continue(state: &Rc<AppState>, entries: Vec<ContinueWatchingEntry>) {
    while let Some(child) = state.continue_box.first_child() {
        child.unparent();
    }
    if entries.is_empty() {
        state.continue_section.set_visible(false);
        return;
    }

    for entry in entries {
        let fraction = match entry.state.duration_ms {
            Some(duration) if duration > 0 => entry.state.position_ms as f64 / duration as f64,
            _ => 0.0,
        };
        let card = build_continue_card(&entry.item, fraction);
        state.continue_box.append(&card.button);

        let id = entry.item.id;
        let state_click = state.clone();
        card.button
            .connect_clicked(move |_| play_item(&state_click, id));
        fill_artwork(state, id, card);
    }

    state.continue_section.set_visible(true);
}

fn fill_artwork(state: &Rc<AppState>, id: i64, card: crate::ui::Card) {
    let api = state.api.clone();
    let card = Rc::new(card);
    state.api.dispatch(
        async move { api.artwork_bytes(id, "poster").await },
        move |result| {
            if let Ok(Some(bytes)) = result {
                let bytes = glib::Bytes::from(&bytes);
                if let Ok(texture) = gdk::Texture::from_bytes(&bytes) {
                    card.picture.set_paintable(Some(&texture));
                    card.stack.set_visible_child_name("art");
                }
            }
        },
    );
}

fn open_detail(state: &Rc<AppState>, id: i64) {
    let api = state.api.clone();
    state.api.dispatch(
        async move {
            let item = api.item(id).await?;
            let progress = api.progress_for_item(id).await.unwrap_or(None);
            Ok::<_, anyhow::Error>((item, progress))
        },
        {
            let state = state.clone();
            move |result| match result {
                Ok((item, progress)) => {
                    let resume_ms = progress
                        .filter(|state| !state.watched && state.position_ms > 0)
                        .map(|state| state.position_ms);
                    let state_play = state.clone();
                    let id = item.id;
                    let page =
                        build_detail_page(&item, resume_ms, move || play_item(&state_play, id));
                    state.nav_view.push(&page);
                }
                Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                Err(err) => toast(&state, &format!("Failed to load item: {err:#}")),
            }
        },
    );
}

fn play_item(state: &Rc<AppState>, item_id: i64) {
    let request = PlaybackStartRequest {
        item_id,
        capabilities: client_capabilities(),
        audio_track: None,
        subtitle_track: None,
        // Omit the position; the server resumes from saved progress.
        start_position_ms: None,
    };
    let api = state.api.clone();
    state
        .api
        .dispatch(async move { api.playback_start(&request).await }, {
            let state = state.clone();
            move |result| match result {
                Ok(session) => start_playback(&state, item_id, session),
                Err(err) if is_unauthorized(&err) => bounce_to_login(&state),
                Err(err) => toast(&state, &format!("Failed to start playback: {err:#}")),
            }
        });
}

fn start_playback(state: &Rc<AppState>, item_id: i64, session: PlaybackSession) {
    if state.player.borrow().is_none() {
        match Player::new(&state.window) {
            Ok(player) => *state.player.borrow_mut() = Some(player),
            Err(err) => {
                toast(state, &format!("Playback unavailable: {err:#}"));
                return;
            }
        }
    }

    let url = state.api.absolute_url(&session.url);
    let load_result = match state.player.borrow_mut().as_mut() {
        Some(player) => player.load(&url).and_then(|()| player.play()),
        None => Err(anyhow::anyhow!("player was not created")),
    };
    if let Err(err) = load_result {
        toast(state, &format!("Failed to start playback: {err:#}"));
        let api = state.api.clone();
        let session_id = session.session_id;
        state
            .api
            .dispatch(async move { api.playback_stop(&session_id).await }, |_| {});
        return;
    }

    *state.active.borrow_mut() = Some(ActivePlayback {
        session_id: session.session_id,
        item_id,
    });
    *state.last_progress.borrow_mut() = Some(Instant::now());

    let backend = state
        .player
        .borrow()
        .as_ref()
        .map(|player| player.backend());
    if let Some(backend) = backend {
        state.player_page.set_status(backend, Some(session.mode));
    }
    state.nav_view.push(&state.player_page.page);
}

/// Stop the active session, optionally persisting progress first.
fn stop_playback(state: &Rc<AppState>, save: bool) {
    let active = state.active.borrow_mut().take();
    let Some(active) = active else {
        return;
    };

    if save {
        save_progress(state, active.item_id);
    } else {
        *state.last_progress.borrow_mut() = None;
    }

    if let Some(player) = state.player.borrow().as_ref() {
        let _ = player.pause();
    }

    let api = state.api.clone();
    let session_id = active.session_id;
    state
        .api
        .dispatch(async move { api.playback_stop(&session_id).await }, |_| {});
}

/// Persist the current position for `item_id` and reset the write throttle.
fn save_progress(state: &Rc<AppState>, item_id: i64) {
    let (position, duration) = match state.player.borrow().as_ref() {
        Some(player) => (player.position(), player.duration()),
        None => return,
    };
    if position <= 0.0 {
        return;
    }

    let request = UpdateProgressRequest {
        position_ms: (position * 1000.0) as u64,
        duration_ms: (duration > 0.0).then_some((duration * 1000.0) as u64),
        watched: None,
    };
    *state.last_progress.borrow_mut() = Some(Instant::now());

    let api = state.api.clone();
    state.api.dispatch(
        async move { api.update_progress(item_id, &request).await },
        {
            let state = state.clone();
            move |result| {
                if let Err(err) = result {
                    if is_unauthorized(&err) {
                        bounce_to_login(&state);
                    }
                }
            }
        },
    );
}

/// Write progress every ~10 s while a session is active (and at most every 5 s).
fn start_progress_timer(state: &Rc<AppState>) {
    let state = state.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        let item_id = state.active.borrow().as_ref().map(|active| active.item_id);
        if let Some(item_id) = item_id {
            let due = state
                .last_progress
                .borrow()
                .map(|last| last.elapsed() >= Duration::from_secs(10))
                .unwrap_or(true);
            if due {
                save_progress(&state, item_id);
            }
        }
        glib::ControlFlow::Continue
    });
}

fn show_status(state: &Rc<AppState>, icon: &str, title: &str, description: &str) {
    // `AdwStatusPage` renders its title/description as Pango markup, so escape
    // any user- or server-supplied text (error bodies can contain `&`).
    state.status_page.set_icon_name(Some(icon));
    state
        .status_page
        .set_title(&glib::markup_escape_text(title));
    state
        .status_page
        .set_description(Some(&glib::markup_escape_text(description)));
    state.browse_stack.set_visible_child_name("status");
}

fn toast(state: &Rc<AppState>, message: &str) {
    state.toast_overlay.add_toast(adw::Toast::new(message));
}
