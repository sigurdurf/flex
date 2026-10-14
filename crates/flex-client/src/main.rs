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
mod ui;

use api::ApiClient;
use gtk4::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "org.flexmedia.FlexClient";

fn main() -> anyhow::Result<()> {
    let server = match parse_args() {
        Ok(server) => server,
        Err(exit) => {
            print!("{}", exit.message);
            std::process::exit(exit.code);
        }
    };

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

    let api = ApiClient::new(server, handle);

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        let window = app::build_ui(app, api.clone());
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

fn parse_args() -> Result<String, EarlyExit> {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        return match arg.as_str() {
            "-h" | "--help" => Err(EarlyExit {
                code: 0,
                message: help_text(),
            }),
            "--version" => Err(EarlyExit {
                code: 0,
                message: format!("flex-client {}\n", env!("CARGO_PKG_VERSION")),
            }),
            "--server" => {
                let value = args.next().ok_or_else(|| EarlyExit {
                    code: 2,
                    message: "--server requires a URL argument\n".to_string(),
                })?;
                Ok(value)
            }
            _ => match arg.strip_prefix("--server=") {
                Some(value) => Ok(value.to_string()),
                None => Err(EarlyExit {
                    code: 2,
                    message: format!("unknown argument: {arg}\n{}", help_text()),
                }),
            },
        };
    }
    Ok(std::env::var("FLEX_SERVER")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{}", flex_protocol::DEFAULT_PORT)))
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
         \x20   -h, --help        Print this help\n\
         \x20   --version         Print the version\n",
        version = env!("CARGO_PKG_VERSION"),
        port = flex_protocol::DEFAULT_PORT,
    )
}
