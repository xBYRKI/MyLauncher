//! Launcher-Konfiguration (Branding, Packs, Server, News) und Benutzereinstellungen.
//!
//! `launcher.config.json` wird in die Exe eingebaut. Eine Datei gleichen Namens in
//! `%APPDATA%\MyLauncher\` überschreibt einzelne Felder, damit man Packs/Server ohne
//! Neu-Bauen ändern kann. Einstellungen liegen in `%APPDATA%\MyLauncher\settings.json`.
//!
//! Ist `remoteConfig` gesetzt, kommen Packs, News und Server zusätzlich vom eigenen Server
//! (Admin-Oberfläche). Reihenfolge: eingebaut → Server → Datei in `%APPDATA%`.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BUNDLED_CONFIG: &str = include_str!("../../../launcher.config.json");
/// Zuletzt vom Server geladene Konfiguration – damit der Launcher auch offline startet.
const REMOTE_CACHE: &str = "remote-config.json";
/// Felder, die der Server nie setzen darf (siehe [`update_config`]).
const REMOTE_BLOCKED: &[&str] = &["update", "remoteConfig"];

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
    /// URL der Launcher-Konfiguration auf dem eigenen Server (`…/launcher/config.json`).
    /// Leer = nur eingebaute und lokale Konfiguration.
    pub remote_config: String,
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

fn bundled_value() -> Value {
    serde_json::from_str(BUNDLED_CONFIG).unwrap_or(Value::Object(Default::default()))
}

/// URL der Server-Konfiguration; die Datei in `%APPDATA%` darf sie ersetzen.
fn remote_url(bundled: &Value, over: Option<&Value>) -> String {
    over.and_then(|o| o["remoteConfig"].as_str())
        .or_else(|| bundled["remoteConfig"].as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn cached_remote(url: &str) -> Option<Value> {
    let cache = read_json(&app_dir().join(REMOTE_CACHE))?;
    (cache["source"].as_str() == Some(url)).then(|| cache["config"].clone())
}

pub fn load_launcher_config() -> LauncherConfig {
    let mut value = bundled_value();
    let over = read_json(&app_dir().join("launcher.config.json"));
    let url = remote_url(&value, over.as_ref());
    if !url.is_empty() {
        if let Some(remote) = cached_remote(&url) {
            merge(&mut value, &remote);
        }
    }
    if let Some(over) = &over {
        merge(&mut value, over);
    }
    serde_json::from_value(value).unwrap_or_default()
}

/// Server-Antwort aufbereiten: gesperrte Felder raus, relative URLs auflösen.
fn clean_remote(mut remote: Value, base: &url::Url) -> Result<Value> {
    let Value::Object(map) = &mut remote else {
        bail!("Server-Konfiguration ist kein JSON-Objekt");
    };
    for key in REMOTE_BLOCKED {
        map.remove(*key);
    }
    if let Some(packs) = map.get_mut("packs").and_then(Value::as_array_mut) {
        for pack in packs {
            if let Some(abs) = pack["manifest"].as_str().and_then(|m| base.join(m).ok()) {
                pack["manifest"] = abs.to_string().into();
            }
        }
    }
    if let Some(abs) = map
        .get("newsUrl")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .and_then(|u| base.join(u).ok())
    {
        map.insert("newsUrl".into(), abs.to_string().into());
    }
    Ok(remote)
}

/// Konfiguration vom Server holen und für [`load_launcher_config`] ablegen.
/// `true`, wenn sie sich seit dem letzten Abruf geändert hat.
pub async fn refresh_remote_config() -> Result<bool> {
    let url = remote_url(
        &bundled_value(),
        read_json(&app_dir().join("launcher.config.json")).as_ref(),
    );
    if url.is_empty() {
        return Ok(false);
    }
    let text = crate::modpack::fetch_source(&url)
        .await
        .with_context(|| format!("Launcher-Konfiguration nicht erreichbar: {url}"))?;
    let remote: Value =
        serde_json::from_str(&text).context("Server-Konfiguration ist kein gültiges JSON")?;
    let remote = clean_remote(remote, &crate::modpack::to_url(&url)?)?;
    if cached_remote(&url).as_ref() == Some(&remote) {
        return Ok(false);
    }
    std::fs::create_dir_all(app_dir())?;
    let cache = serde_json::json!({
        "source": url,
        "fetchedAt": crate::now_millis(),
        "config": remote,
    });
    std::fs::write(
        app_dir().join(REMOTE_CACHE),
        serde_json::to_string_pretty(&cache)?,
    )?;
    Ok(true)
}

/// Update-Einstellungen – absichtlich **nur** aus der eingebauten Config.
///
/// Die Datei in `%APPDATA%` darf hier nicht mitreden: wer Feed-URL und Thumbprint
/// ersetzen kann, kann dem Launcher jede beliebige Exe als Update unterschieben.
/// Zum Testen gegen einen anderen Feed gibt es `MYLAUNCHER_UPDATE_FEED`.
pub fn update_config() -> UpdateConfig {
    let value = bundled_value();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_darf_update_nicht_setzen() {
        let base = url::Url::parse("https://mc.example.net/launcher/config.json").unwrap();
        let remote = json!({
            "update": { "feed": "https://boese.example/update.json", "signerThumbprint": "00" },
            "remoteConfig": "https://boese.example/config.json",
            "packs": [
                { "manifest": "packs/survival/manifest.json" },
                { "manifest": "https://cdn.example.net/x/manifest.json" }
            ],
            "newsUrl": "news.json"
        });
        let clean = clean_remote(remote, &base).unwrap();
        assert!(clean.get("update").is_none());
        assert!(clean.get("remoteConfig").is_none());
        assert_eq!(
            clean["packs"][0]["manifest"],
            "https://mc.example.net/launcher/packs/survival/manifest.json"
        );
        assert_eq!(clean["packs"][1]["manifest"], "https://cdn.example.net/x/manifest.json");
        assert_eq!(clean["newsUrl"], "https://mc.example.net/launcher/news.json");
    }

    #[test]
    fn nur_objekte_werden_angenommen() {
        let base = url::Url::parse("https://mc.example.net/").unwrap();
        assert!(clean_remote(json!([1, 2]), &base).is_err());
    }
}
