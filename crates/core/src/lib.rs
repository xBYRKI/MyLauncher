//! Launcher-Logik ohne UI: Prism-Brücke, Modpack-Sync, Server-Ping und Xaero-Wegpunkt-Sync.
//! Wird von der Tauri-App und vom Hook-Modus (`--xaero-sync`) gemeinsam genutzt.

pub mod authenticode;
pub mod config;
pub mod ini;
pub mod modpack;
pub mod ping;
pub mod prism;
pub mod transfer;
pub mod update;
pub mod waypoints;
pub mod xaero;

pub const USER_AGENT: &str = concat!("MyLauncher/", env!("CARGO_PKG_VERSION"));

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
