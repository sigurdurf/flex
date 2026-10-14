//! The login page shown when no valid session token is available.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Label, Orientation, PasswordEntry, Spinner};
use libadwaita as adw;

/// Widgets and handles for the login form.
pub struct LoginPage {
    /// The top-level widget to place in the window's stack.
    pub widget: adw::ToolbarView,
    pub server_entry: Entry,
    pub username_entry: Entry,
    pub password_entry: PasswordEntry,
    pub device_entry: Entry,
    pub login_button: Button,
    pub spinner: Spinner,
    pub status_label: Label,
}

impl LoginPage {
    /// Build a login form pre-filled with `server`.
    pub fn new(server: &str) -> Self {
        let heading = Label::new(Some("Sign in to Flex"));
        heading.add_css_class("title-1");

        let subtitle = Label::new(Some("Enter your server and account credentials."));
        subtitle.add_css_class("dim-label");
        subtitle.set_wrap(true);

        let server_entry = Entry::new();
        server_entry.set_text(server);
        server_entry.set_placeholder_text(Some("http://host:8096"));
        server_entry.set_hexpand(true);

        let username_entry = Entry::new();
        username_entry.set_placeholder_text(Some("Username"));
        username_entry.set_hexpand(true);
        username_entry.set_activates_default(true);

        let password_entry = PasswordEntry::new();
        password_entry.set_placeholder_text(Some("Password"));
        password_entry.set_hexpand(true);
        password_entry.set_show_peek_icon(true);
        password_entry.set_activates_default(true);

        let device_entry = Entry::new();
        device_entry.set_placeholder_text(Some("Device name (optional)"));
        device_entry.set_hexpand(true);
        device_entry.set_activates_default(true);

        let login_button = Button::with_label("Sign in");
        login_button.add_css_class("suggested-action");
        login_button.add_css_class("pill");
        login_button.set_halign(Align::Center);
        login_button.set_margin_top(6);

        let spinner = Spinner::new();
        spinner.set_visible(false);

        let status_label = Label::new(None);
        status_label.add_css_class("error");
        status_label.set_wrap(true);
        status_label.set_visible(false);
        status_label.set_xalign(0.0);

        let form = GtkBox::new(Orientation::Vertical, 12);
        form.set_valign(Align::Center);
        form.append(&heading);
        form.append(&subtitle);
        form.append(&labeled("Server URL", &server_entry));
        form.append(&labeled("Username", &username_entry));
        form.append(&labeled("Password", &password_entry));
        form.append(&labeled("Device name", &device_entry));
        form.append(&status_label);
        form.append(&login_button);
        form.append(&spinner);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(420);
        clamp.set_child(Some(&form));

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.append(&clamp);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&content));

        Self {
            widget: toolbar,
            server_entry,
            username_entry,
            password_entry,
            device_entry,
            login_button,
            spinner,
            status_label,
        }
    }

    /// Show an inline status/error message.
    pub fn set_status(&self, message: &str) {
        if message.is_empty() {
            self.status_label.set_visible(false);
        } else {
            self.status_label.set_text(message);
            self.status_label.set_visible(true);
        }
    }

    /// Toggle the busy state while a login request is in flight.
    pub fn set_busy(&self, busy: bool) {
        self.login_button.set_sensitive(!busy);
        self.spinner.set_visible(busy);
        if busy {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
    }
}

fn labeled(text: &str, entry: &impl IsA<gtk4::Widget>) -> GtkBox {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("dim-label");
    let row = GtkBox::new(Orientation::Vertical, 4);
    row.append(&label);
    row.append(entry);
    row
}
