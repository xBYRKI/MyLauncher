//! Launcher-Konfiguration (Branding, Packs, Server, News) und Benutzereinstellungen.
//!
//! `launcher.config.json` wird in die Exe eingebaut. Eine Datei gleichen Namens in
//! `%APPDATA%\MyLauncher\` überschreibt einzelne Felder, damit man Packs/Server ohne
//! Neu-Bauen ändern kann. Einstellungen liegen in `%APPDATA%\MyLauncher\settings.json`.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BUNDLED_CONFIG: &str = include_str!("../../../launcher.config.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Brand {
    pub name: String,
    pub tagline: String,
    pub accent: String,
}

impl Default for Brand {
    fn default() -> Self {
        Self {
            name: "MyLauncher".into(),
            tagline: "Powered by Prism".into(),
            accent: "#7c5cff".into(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PackRef {
    pub manifest: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerRef {
    pub name: String,
    pub address: String,
    /// Optional: Ordnername einer Prism-Instanz für den "Beitreten"-Button.
    pub instance: Option<String>,
}

/// Selbst-Update. Wird **nur** aus der eingebauten Config gelesen – siehe [`update_config`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateConfig {
    /// URL einer `update.json`. Leer = Selbst-Update aus.
    pub feed: String,
    /// SHA1-Thumbprint unseres Code-Signing-Zertifikats. Ohne ihn wird nicht aktualisiert.
    pub signer_thumbprint: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LauncherConfig {
    pub brand: Brand,
    pub news_url: String,
    pub news: Vec<Value>,
    pub packs: Vec<PackRef>,
    pub servers: Vec<ServerRef>,
    /// Prism-Instanzen (Ordnernamen), die im Launcher nicht angezeigt werden.
    pub hidden_instances: Vec<String>,
    /// Vorgabe für den Wegpunkt-Sync-Server, falls in den Einstellungen leer.
    pub waypoint_server: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WaypointSettings {
    pub server_url: String,
    /// Team-Schlüssel, muss zum Token auf dem Sync-Server passen.
    pub token: String,
    /// Nur Wegpunkte in diesem Xaero-Set werden geteilt.
    pub shared_set: String,
    /// Alle Wegpunkte teilen (statt nur das Set).
    pub share_all: bool,
    /// Xaero-Weltordner, die synchronisiert werden (z. B. `Multiplayer_play.example.net`).
    pub worlds: Vec<String>,
}

impl Default for WaypointSettings {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            token: String::new(),
            shared_set: "Team".into(),
            share_all: false,
            worlds: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub prism_exe: String,
    pub prism_data_dir: String,
    /// Prism-Profilname, leer = Prism-Standardkonto.
    pub account: String,
    /// `minimize` | `keep` | `close`
    pub after_launch: String,
    pub auto_update_packs: bool,
    /// Beim Start und im Hintergrund nach einer neuen Launcher-Version sehen.
    pub auto_update_launcher: bool,
    pub join_server: bool,
    pub waypoints: WaypointSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            prism_exe: String::new(),
            prism_data_dir: String::new(),
            account: String::new(),
            after_launch: "minimize".into(),
            auto_update_packs: true,
            auto_update_launcher: true,
            join_server: true,
            waypoints: WaypointSettings::default(),
        }
    }
}

/// `%APPDATA%\MyLauncher` – oder `MYLAUNCHER_HOME` (portabel / Tests).
pub fn app_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("MYLAUNCHER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(home);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("MyLauncher")
}

fn read_json(path: &PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Objekte rekursiv zusammenführen, alles andere ersetzen.
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p.clone(),
    }
}

pub fn load_launcher_config() -> LauncherConfig {
    let mut value: Value =
        serde_json::from_str(BUNDLED_CONFIG).unwrap_or(Value::Object(Default::default()));
    if let Some(over) = read_json(&app_dir().join("launcher.config.json")) {
        merge(&mut value, &over);
    }
    serde_json::from_value(value).unwrap_or_default()
}

/// Update-Einstellungen – absichtlich **nur** aus der eingebauten Config.
///
/// Die Datei in `%APPDATA%` darf hier nicht mitreden: wer Feed-URL und Thumbprint
/// ersetzen kann, kann dem Launcher jede beliebige Exe als Update unterschieben.
/// Zum Testen gegen einen anderen Feed gibt es `MYLAUNCHER_UPDATE_FEED`.
pub fn update_config() -> UpdateConfig {
    let value: Value =
        serde_json::from_str(BUNDLED_CONFIG).unwrap_or(Value::Object(Default::default()));
    let mut cfg: UpdateConfig =
        serde_json::from_value(value["update"].clone()).unwrap_or_default();
    if let Some(feed) = std::env::var_os("MYLAUNCHER_UPDATE_FEED") {
        cfg.feed = feed.to_string_lossy().into_owned();
    }
    cfg
}

pub fn load_settings() -> Settings {
    read_json(&app_dir().join("settings.json"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(app_dir())?;
    std::fs::write(
        app_dir().join("settings.json"),
        serde_json::to_string_pretty(settings)?,
    )?;
    Ok(())
}

/// Teil-Update aus der UI übernehmen und speichern.
pub fn patch_settings(patch: &Value) -> Result<Settings> {
    let mut value = serde_json::to_value(load_settings())?;
    merge(&mut value, patch);
    let settings: Settings = serde_json::from_value(value)?;
    save_settings(&settings)?;
    Ok(settings)
}

/// Effektive Sync-Server-URL (Einstellung oder Vorgabe aus der Launcher-Config).
pub fn waypoint_server_url(settings: &Settings, cfg: &LauncherConfig) -> String {
    let url = if settings.waypoints.server_url.trim().is_empty() {
        cfg.waypoint_server.trim()
    } else {
        settings.waypoints.server_url.trim()
    };
    url.trim_end_matches('/').to_string()
}
