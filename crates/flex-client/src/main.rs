//! # flex-client
//!
//! A GTK4 + libadwaita desktop client for the Flex media server. It browses
//! libraries over the Flex HTTP API and plays media through libmpv (embedded on
//! X11/XWayland) or an external `mpv` process (Wayland fallback).
//!
//! Run with `flex-client --server http://host:8096` (or set `FLEX_SERVER`).

#![forbid(unsafe_code)]

mod api;
mod app;
mod player;
mod session;
mod ui;

use api::ApiClient;
use gtk4::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "org.flexmedia.FlexClient";

fn main() -> anyhow::Result<()> {
    let (server, should_logout) = match parse_args() {
        Ok(opts) => opts,
        Err(exit) => {
            print!("{}", exit.message);
            std::process::exit(exit.code);
        }
    };

    if should_logout {
        let _ = session::clear();
    }

    // Networking runs on a dedicated Tokio runtime. Move the runtime into a
    // parked thread so it (and its worker pool) outlive `main`'s scope.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    std::thread::Builder::new()
        .name("flex-tokio".into())
        .spawn(move || {
            let _runtime = runtime;
            std::thread::park();
        })?;

    // Try to load stored session if it matches server
    let stored_session = session::load().ok().flatten();
    let effective_server = if let Some(ref sess) = stored_session {
        if sess.server_url == server {
            sess.server_url.clone()
        } else {
            server.clone()
        }
    } else {
        server.clone()
    };

    let api = ApiClient::new(effective_server, handle.clone());
    if let Some(sess) = stored_session {
        if sess.server_url == server {
            api.set_token(Some(sess.token.clone()));
        }
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    let api_clone = api.clone();
    app.connect_activate(move |app| {
        let window = app::build_ui(app, api_clone.clone());
        window.present();
    });

    // Pass only argv[0]: our own CLI flags (`--server`, …) are consumed above
    // and GApplication would otherwise reject them as unknown options.
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "flex-client".to_string());
    let code = app.run_with_args(&[program]);
    std::process::exit(i32::from(code));
}

struct EarlyExit {
    code: i32,
    message: String,
}

fn parse_args() -> Result<(String, bool), EarlyExit> {
    let mut args = std::env::args().skip(1);
    let mut logout = false;
    let mut server: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Err(EarlyExit {
                    code: 0,
                    message: help_text(),
                })
            }
            "--version" => {
                return Err(EarlyExit {
                    code: 0,
                    message: format!("flex-client {}\n", env!("CARGO_PKG_VERSION")),
                })
            }
            "--logout" => {
                logout = true;
            }
            "--server" => {
                let value = args.next().ok_or_else(|| EarlyExit {
                    code: 2,
                    message: "--server requires a URL argument\n".to_string(),
                })?;
                server = Some(value);
            }
            _ => match arg.strip_prefix("--server=") {
                Some(value) => {
                    server = Some(value.to_string());
                }
                None => {
                    return Err(EarlyExit {
                        code: 2,
                        message: format!("unknown argument: {arg}\n{}", help_text()),
                    })
                }
            },
        }
    }

    let server = match server {
        Some(s) => s,
        None => std::env::var("FLEX_SERVER")
            .unwrap_or_else(|_| format!("http://127.0.0.1:{}", flex_protocol::DEFAULT_PORT)),
    };
    Ok((server, logout))
}

fn help_text() -> String {
    format!(
        "flex-client {version}\n\
         \n\
         A GTK4/libadwaita desktop client for the Flex media server.\n\
         \n\
         USAGE:\n\
         \x20   flex-client [OPTIONS]\n\
         \n\
         OPTIONS:\n\
          \x20   --server <URL>    Flex server base URL (env: FLEX_SERVER)\n\
          \x20                     [default: http://127.0.0.1:{port}]\n\
          \x20   --logout          Clear stored session credentials\n\
          \x20   -h, --help        Print this help\n\
          \x20   --version         Print the version\n",
        version = env!("CARGO_PKG_VERSION"),
        port = flex_protocol::DEFAULT_PORT,
    )
}
