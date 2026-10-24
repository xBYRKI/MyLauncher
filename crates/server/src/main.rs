//! Server für MyLauncher: Modpacks, Launcher-Updates und Admin-Oberfläche.
//!
//! ```text
//! mylauncher-server --bind 0.0.0.0:8787 --data ./data \
//!     --admin-password LANGES-ADMIN-PASSWORT --public-url https://mc.example.net
//! ```
//! Alternativ per Umgebung: `WAYPOINT_BIND`, `WAYPOINT_DATA` (Namen aus der Zeit des Wegpunkt-Servers),
//! `MYLAUNCHER_ADMIN_PASSWORD`, `MYLAUNCHER_PUBLIC_URL`.
//! Für Internet-Betrieb einen Reverse-Proxy mit HTTPS davorschalten (Caddy, nginx).
//!
//! | Pfad | |
//! |---|---|
//! | `/launcher/…` | Packs, News, Server, Update-Feed für den Launcher (öffentlich) |
//! | `/admin/` | Admin-Oberfläche (Admin-Passwort) |

mod admin;
mod public;
mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use tokio::sync::Mutex;

pub struct AppState {
    store: Mutex<store::Store>,
    auth: admin::Auth,
    /// Öffentliche Adresse ohne `/` am Ende; leer = aus den Anfrage-Headern ableiten.
    public_url: String,
}

pub type Shared = Arc<AppState>;
pub type ApiError = (StatusCode, String);

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn bad(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::BAD_REQUEST, format!("{e:#}"))
}

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    eprintln!("Fehler: {e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
}

/// Adresse, unter der Launcher den Server erreichen – für absolute URLs (Update-Feed).
/// Mit `--public-url` fest vorgegeben, sonst aus `X-Forwarded-*`/`Host`.
pub fn public_base(state: &AppState, headers: &HeaderMap) -> String {
    if !state.public_url.is_empty() {
        return state.public_url.clone();
    }
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let proto = get("x-forwarded-proto")
        .filter(|p| p == "https" || p == "http")
        .unwrap_or_else(|| "http".into());
    let host = get("x-forwarded-host")
        .or_else(|| get(header::HOST.as_str()))
        .unwrap_or_else(|| "localhost".into());
    format!("{proto}://{host}")
}

struct Args {
    bind: SocketAddr,
    data: PathBuf,
    admin_password: String,
    public_url: String,
}

const USAGE: &str = "mylauncher-server --bind 0.0.0.0:8787 --data ./data --admin-password PASSWORT \
[--public-url https://mc.example.net]";

fn parse_args() -> Result<Args> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let mut bind = std::env::var("WAYPOINT_BIND").unwrap_or_else(|_| "0.0.0.0:8787".into());
    let mut data = std::env::var("WAYPOINT_DATA").unwrap_or_else(|_| "data".into());
    let mut admin_password = env("MYLAUNCHER_ADMIN_PASSWORD");
    let mut public_url = env("MYLAUNCHER_PUBLIC_URL");

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next()
                .with_context(|| format!("{a} braucht einen Wert"))
        };
        match a.as_str() {
            "--bind" => bind = value()?,
            "--data" => data = value()?,
            "--admin-password" => admin_password = value()?,
            "--public-url" => public_url = value()?,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("mylauncher-server {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => bail!("Unbekanntes Argument: {other}"),
        }
    }

    if admin_password.is_empty() {
        bail!("--admin-password fehlt\n{USAGE}");
    }
    if admin_password.chars().count() < 12 {
        bail!("Admin-Passwort ist zu kurz (mind. 12 Zeichen)");
    }
    let public_url = public_url.trim().trim_end_matches('/').to_string();
    if !public_url.is_empty()
        && !public_url.starts_with("https://")
        && !public_url.starts_with("http://")
    {
        bail!("--public-url muss mit http:// oder https:// beginnen");
    }
    Ok(Args {
        bind: bind.parse().context("Ungültige --bind-Adresse")?,
        data: PathBuf::from(data),
        admin_password,
        public_url,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    std::fs::create_dir_all(&args.data)?;
    let store = store::Store::open(&args.data)?;
    println!(
        "MyLauncher-Server auf http://{} – Daten in {}",
        args.bind,
        args.data.display()
    );
    println!(
        "  Modpacks: {} · Admin: http://{}/admin/",
        store.packs.len(),
        args.bind
    );

    let state = Arc::new(AppState {
        store: Mutex::new(store),
        auth: admin::Auth::new(args.admin_password),
        public_url: args.public_url,
    });
    let app = Router::new()
        .route("/", get(|| async { Redirect::to("/admin/") }))
        .route("/admin", get(|| async { Redirect::permanent("/admin/") }))
        .route("/admin/", get(admin::index))
        .nest("/launcher", public::router())
        .nest("/admin", admin::router())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
