//! Administrative `flex-server user ...` subcommands.
//!
//! Each action opens the configured database, performs a single mutation and
//! exits, printing a clear message. Errors bubble up as a non-zero exit status.

use std::io::{self, Write};

use flex_protocol::UserRole;

use crate::auth;
#[cfg(windows)]
use crate::config::ServiceAction;
use crate::config::{Cli, Command, UserAction, UserCommand};
use crate::db;

/// Run a parsed subcommand against the configured database.
pub async fn run(command: Command, cli: &Cli) -> anyhow::Result<()> {
    match command {
        Command::User(UserCommand { action }) => run_user(action, cli).await,
        #[cfg(windows)]
        Command::Service(svc) => run_service(svc.action),
    }
}

async fn run_user(action: UserAction, cli: &Cli) -> anyhow::Result<()> {
    let (config, _) = crate::config::load(cli)?;
    let pool = db::init_pool(&config.database).await?;

    match action {
        UserAction::Add {
            username,
            admin,
            password,
        } => {
            let username = require_username(&username)?;
            if db::find_user_by_username(&pool, &username).await?.is_some() {
                anyhow::bail!("user {username:?} already exists");
            }
            let password = resolve_password(password)?;
            let hash = hash_or_bail(&password)?;
            let role = if admin {
                UserRole::Admin
            } else {
                UserRole::User
            };
            let user = db::create_user(&pool, &username, &hash, role).await?;
            println!(
                "created {} user {:?} (id {})",
                role_name(user.role),
                user.username,
                user.id
            );
        }
        UserAction::List => {
            let users = db::list_users(&pool).await?;
            if users.is_empty() {
                println!("no users");
            } else {
                println!(
                    "{:<6} {:<24} {:<6} {:<12}",
                    "ID", "USERNAME", "ROLE", "CREATED"
                );
                for user in users {
                    println!(
                        "{:<6} {:<24} {:<6} {:<12}",
                        user.id,
                        user.username,
                        role_name(user.role),
                        user.created_at
                    );
                }
            }
        }
        UserAction::Passwd { username, password } => {
            let username = require_username(&username)?;
            let password = resolve_password(password)?;
            let hash = hash_or_bail(&password)?;
            if db::update_password(&pool, &username, &hash).await? {
                println!("password updated for {username:?}");
            } else {
                anyhow::bail!("user {username:?} not found");
            }
        }
        UserAction::Delete { username } => {
            let username = require_username(&username)?;
            if db::delete_user(&pool, &username).await? {
                println!("deleted user {username:?}");
            } else {
                anyhow::bail!("user {username:?} not found");
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn run_service(action: ServiceAction) -> anyhow::Result<()> {
    use crate::windows_service;
    match action {
        ServiceAction::Install {
            name,
            display,
            description,
            config,
            working_dir,
            database,
            cache_dir,
            start,
        } => {
            windows_service::install(
                &name,
                &display,
                &description,
                config,
                working_dir,
                database,
                cache_dir,
            )?;
            if start {
                windows_service::start(&name)?;
            }
            Ok(())
        }
        ServiceAction::Uninstall { name, force } => windows_service::uninstall(&name, force),
        ServiceAction::Run { name } => {
            let _ = name;
            windows_service::run()
        }
        ServiceAction::Start { name } => windows_service::start(&name),
        ServiceAction::Stop { name } => windows_service::stop(&name),
        ServiceAction::Status { name } => windows_service::status(&name),
    }
}

fn require_username(username: &str) -> anyhow::Result<String> {
    let username = username.trim().to_string();
    if username.is_empty() {
        anyhow::bail!("username must not be empty");
    }
    Ok(username)
}

fn role_name(role: UserRole) -> &'static str {
    match role {
        UserRole::Admin => "admin",
        UserRole::User => "user",
    }
}

fn hash_or_bail(password: &str) -> anyhow::Result<String> {
    auth::hash_password(password).map_err(|e| anyhow::anyhow!("failed to hash password: {e}"))
}

/// Resolve a password from `--password`, then `$FLEX_PASSWORD`, then stdin.
///
/// Note: the stdin prompt does not suppress echo.
fn resolve_password(flag: Option<String>) -> anyhow::Result<String> {
    if let Some(password) = flag {
        if !password.is_empty() {
            return Ok(password);
        }
    }
    if let Ok(password) = std::env::var("FLEX_PASSWORD") {
        if !password.is_empty() {
            return Ok(password);
        }
    }

    eprint!("Password: ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        anyhow::bail!("password must not be empty");
    }
    Ok(password)
}
