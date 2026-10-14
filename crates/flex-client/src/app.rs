//! Window construction and UI wiring.
//!
//! The whole browser is built in code (no `.gresource`/blueprint files). The
//! layout is an [`adw::NavigationSplitView`] whose sidebar lists the libraries
//! and whose content hosts a [`adw::NavigationView`] onto which detail and
//! player pages are pushed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use flex_protocol::{ItemQuery, Library, MediaItem};
use gtk4::prelude::*;
use gtk4::{gdk, glib, Align, Box as GtkBox, Button, Orientation, SelectionMode};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::api::ApiClient;
use crate::player::Player;
use crate::ui::{build_card, build_detail_page, PlayerPage};

/// Shared application state. Widgets are reference-counted GObjects, so cloning
/// the handles into signal closures is cheap.
pub struct AppState {
    api: ApiClient,
    window: gtk4::Window,
    nav_view: adw::NavigationView,
    flow: gtk4::FlowBox,
    browse_stack: gtk4::Stack,
    status_page: adw::StatusPage,
    sidebar_list: gtk4::ListBox,
    search: gtk4::SearchEntry,
    toast_overlay: adw::ToastOverlay,
    library_ids: RefCell<Vec<Option<i64>>>,
    current_library: Cell<Option<i64>>,
    player: Rc<RefCell<Option<Player>>>,
    player_page: Rc<PlayerPage>,
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

    let status_page = adw::StatusPage::new();
    status_page.set_vexpand(true);
    status_page.set_icon_name(Some("folder-videos-symbolic"));
    status_page.set_title("Loading…");

    let browse_stack = gtk4::Stack::new();
    browse_stack.add_named(&flow_scroll, Some("grid"));
    browse_stack.add_named(&status_page, Some("status"));
    browse_stack.set_visible_child_name("status");

    let browse_header = adw::HeaderBar::new();
    let browse_title = adw::WindowTitle::new("Browse", api.base_url());
    browse_header.set_title_widget(Some(&browse_title));

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
    window.set_content(Some(&toast_overlay));

    window.connect_close_request({
        let player = player.clone();
        move |_| {
            if let Some(player) = player.borrow().as_ref() {
                let _ = player.pause();
            }
            glib::Propagation::Proceed
        }
    });

    let state = Rc::new(AppState {
        api,
        window: window_widget,
        nav_view,
        flow,
        browse_stack,
        status_page,
        sidebar_list: sidebar_list.clone(),
        search: search.clone(),
        toast_overlay: toast_overlay.clone(),
        library_ids: RefCell::new(Vec::new()),
        current_library: Cell::new(None),
        player: player.clone(),
        player_page: player_page.clone(),
    });

    connect_signals(&state, &scan_button);
    attach_keyboard_shortcuts(&state);

    state.player_page.start_polling();
    check_health(&state);
    load_libraries(&state);
    refresh_items(&state);

    window
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

fn connect_signals(state: &Rc<AppState>, scan_button: &Button) {
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
                    Err(err) => toast(&state, &format!("Scan failed: {err:#}")),
                }
            });
        }
    });

    state.sidebar_list.connect_row_activated({
        let state = state.clone();
        move |_, row| {
            let index = row.index().max(0) as usize;
            let id = state.library_ids.borrow().get(index).copied().flatten();
            state.current_library.set(id);
            refresh_items(&state);
        }
    });

    state.search.connect_search_changed({
        let state = state.clone();
        move |_| refresh_items(&state)
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
    ids.push(None);

    let all_row = adw::ActionRow::builder().title("All").build();
    all_row.add_prefix(&gtk4::Image::from_icon_name("view-grid-symbolic"));
    state.sidebar_list.append(&all_row);

    for library in libraries {
        ids.push(Some(library.id));
        let row = adw::ActionRow::builder()
            .title(library.name.clone())
            .subtitle(format!("{} items", library.item_count))
            .build();
        row.add_prefix(&gtk4::Image::from_icon_name(&library_icon(library)));
        state.sidebar_list.append(&row);
    }

    *state.library_ids.borrow_mut() = ids;
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

        let button = card.button.clone();
        let id = item.id;
        let state_click = state.clone();
        button.connect_clicked(move |_| open_detail(&state_click, id));

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

    state.browse_stack.set_visible_child_name("grid");
}

fn open_detail(state: &Rc<AppState>, id: i64) {
    let api = state.api.clone();
    state.api.dispatch(async move { api.item(id).await }, {
        let state = state.clone();
        move |result| match result {
            Ok(item) => {
                let url = state.api.stream_url(item.id);
                let state_play = state.clone();
                let page = build_detail_page(&item, move || play_url(&state_play, &url));
                state.nav_view.push(&page);
            }
            Err(err) => toast(&state, &format!("Failed to load item: {err:#}")),
        }
    });
}

fn play_url(state: &Rc<AppState>, url: &str) {
    if state.player.borrow().is_none() {
        match Player::new(&state.window) {
            Ok(player) => *state.player.borrow_mut() = Some(player),
            Err(err) => {
                toast(state, &format!("Playback unavailable: {err:#}"));
                return;
            }
        }
    }

    let load_result = match state.player.borrow_mut().as_mut() {
        Some(player) => player.load(url).and_then(|()| player.play()),
        None => Err(anyhow::anyhow!("player was not created")),
    };
    if let Err(err) = load_result {
        toast(state, &format!("Failed to start playback: {err:#}"));
        return;
    }

    let backend = state
        .player
        .borrow()
        .as_ref()
        .map(|player| player.backend());
    if let Some(backend) = backend {
        state.player_page.set_status(backend);
    }
    state.nav_view.push(&state.player_page.page);
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
