//! Aufgaben für die Oberfläche. Die eigentliche Logik steckt in `launcher-core`;
//! hier wird sie nur so zugeschnitten, wie die UI sie braucht.

use std::time::Duration;

use launcher_core::config::{self, Settings};
use launcher_core::modpack::{self, PackPlan, PackStatus, Progress, SyncResult};
use launcher_core::prism::{self, Account, Instance, Prism};
use launcher_core::transfer::{self, Parts, Summary, WorldTargets};
use launcher_core::update::{self, UpdateStatus};
use launcher_core::waypoints::{self, SyncOptions};
use serde_json::Value;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub type Res<T> = Result<T, String>;

pub fn err(e: impl std::fmt::Display) -> String {
    // anyhow mit Ursachenkette, ohne Debug-Rauschen
    format!("{e:#}")
}

pub fn current_prism() -> Prism {
    prism::resolve(&config::load_settings())
}

fn hidden() -> Vec<String> {
    config::load_launcher_config().hidden_instances
}

fn ensure_configured_pack(source: &str) -> Res<()> {
    // Nur in der Config eingetragene Manifeste dürfen in Prism installiert werden.
    if config::load_launcher_config()
        .packs
        .iter()
        .any(|p| p.manifest == source)
    {
        Ok(())
    } else {
        Err("Unbekanntes Modpack".into())
    }
}

// ---------- Allgemein ----------

pub fn save_settings(patch: &Value) -> Res<Settings> {
    config::patch_settings(patch).map_err(err)
}

pub fn open_external(url: &str) -> Res<()> {
    if url.starts_with("https://") || url.starts_with("http://") {
        opener::open_browser(url).map_err(err)
    } else {
        Err("Nur http(s)-Links".into())
    }
}

pub async fn pick_file() -> Option<String> {
    let mut dialog = rfd::AsyncFileDialog::new().set_title("Prism Launcher auswählen");
    if cfg!(windows) {
        dialog = dialog.add_filter("Prism Launcher", &["exe"]);
    }
    dialog
        .pick_file()
        .await
        .map(|f| f.path().to_string_lossy().into_owned())
}

pub async fn pick_dir() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .pick_folder()
        .await
        .map(|f| f.path().to_string_lossy().into_owned())
}

/// Bild für Avatar oder News laden. Nur https, nur kleine Dateien.
pub async fn fetch_image(url: &str) -> Res<Vec<u8>> {
    if !url.starts_with("https://") {
        return Err("Nur https-Bilder".into());
    }
    let client = reqwest::Client::builder()
        .user_agent(launcher_core::USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(err)?;
    let bytes = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(err)?
        .bytes()
        .await
        .map_err(err)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Bild zu groß".into());
    }
    Ok(bytes.to_vec())
}

// ---------- Prism ----------

pub async fn prism_and_accounts() -> (Prism, Vec<Account>) {
    tokio::task::spawn_blocking(|| {
        let p = current_prism();
        let accounts = prism::list_accounts(&p);
        (p, accounts)
    })
    .await
    .expect("Prism-Status")
}

pub async fn instances() -> Vec<Instance> {
    tokio::task::spawn_blocking(|| prism::list_instances(&current_prism(), &hidden()))
        .await
        .unwrap_or_default()
}

/// Ohne Prism-Hooks wenigstens vor dem Start die geteilten Wegpunkte holen.
async fn pull_waypoints_if_needed(p: &Prism, instance_id: &str) {
    let settings = config::load_settings();
    if settings.waypoints.worlds.is_empty() {
        return;
    }
    let Ok(dir) = prism::instance_dir(p, instance_id) else {
        return;
    };
    if waypoints::hooks_installed(&launcher_core::ini::Ini::load(&dir.join("instance.cfg"))) {
        return; // Prism erledigt das per PreLaunchCommand
    }
    let cfg = config::load_launcher_config();
    if let Ok(opts) = SyncOptions::new(&settings, &cfg, prism::player_name(p, &settings)) {
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            waypoints::sync_game_dir(&prism::game_dir(&dir), &settings.waypoints.worlds, &opts),
        )
        .await;
    }
}

pub async fn launch(instance_id: String, server: Option<String>) -> Res<()> {
    let settings = config::load_settings();
    let p = current_prism();
    pull_waypoints_if_needed(&p, &instance_id).await;
    let server = if settings.join_server {
        server.unwrap_or_default()
    } else {
        String::new()
    };
    prism::launch(&p, &instance_id, &settings.account, &server).map_err(err)
}

pub fn show_in_prism(instance_id: Option<&str>) -> Res<()> {
    prism::show(&current_prism(), instance_id).map_err(err)
}

pub fn open_folder(instance_id: Option<&str>) -> Res<()> {
    let p = current_prism();
    let target = match instance_id {
        Some(id) => prism::instance_dir(&p, id).map_err(err)?,
        None => p.data_dir,
    };
    opener::open(&target).map_err(err)
}

// ---------- Modpacks ----------

pub async fn packs() -> Vec<PackStatus> {
    let p = current_prism();
    let sources: Vec<String> = config::load_launcher_config()
        .packs
        .into_iter()
        .map(|p| p.manifest)
        .collect();
    let mut out = vec![];
    for s in sources {
        out.push(modpack::pack_status(&p, &s).await);
    }
    out
}

pub async fn sync_pack(
    source: &str,
    on_progress: impl Fn(Progress) + Send + Sync + 'static,
) -> Res<SyncResult> {
    ensure_configured_pack(source)?;
    let brand = config::load_launcher_config().brand.name;
    modpack::sync_pack(&current_prism(), source, &brand, on_progress)
        .await
        .map_err(err)
}

/// Abgleich ohne Schreibzugriff: was würde ein Sync ändern, stimmen alle Dateien noch,
/// welche Mods hat der Spieler selbst dazugelegt.
pub async fn pack_plan(
    source: &str,
    on_progress: impl Fn(Progress) + Send + Sync,
) -> Res<PackPlan> {
    ensure_configured_pack(source)?;
    modpack::plan_pack(&current_prism(), source, on_progress)
        .await
        .map_err(err)
}

pub async fn play_pack(
    source: &str,
    on_progress: impl Fn(Progress) + Send + Sync + 'static,
) -> Res<SyncResult> {
    let settings = config::load_settings();
    let p = current_prism();
    let result = if settings.auto_update_packs {
        sync_pack(source, on_progress).await?
    } else {
        ensure_configured_pack(source)?;
        let (m, _) = modpack::load_manifest(source).await.map_err(err)?;
        SyncResult {
            instance_id: m.instance_id,
            version: m.version,
            server: m.server,
            downloaded: 0,
            removed: 0,
        }
    };
    pull_waypoints_if_needed(&p, &result.instance_id).await;
    let server = if settings.join_server {
        result.server.clone()
    } else {
        String::new()
    };
    prism::launch(&p, &result.instance_id, &settings.account, &server).map_err(err)?;
    Ok(result)
}

// ---------- Launcher-Update ----------

pub async fn update_status() -> UpdateStatus {
    update::check(VERSION).await
}

/// Lädt das Update, prüft Prüfsumme und Signatur und startet den Installer.
/// Gibt die Version zurück, die installiert wird – danach muss der Launcher sich beenden,
/// sonst kann der Installer die laufende Exe nicht ersetzen.
pub async fn update_install(on_progress: impl Fn(update::Progress)) -> Res<String> {
    let cfg = config::update_config();
    if cfg.feed.trim().is_empty() {
        return Err("Für diesen Launcher ist kein Update-Server hinterlegt.".into());
    }
    let feed = update::fetch_feed(&cfg.feed).await.map_err(err)?;
    if !update::is_newer(&feed.version, VERSION) {
        return Err(format!("Version {VERSION} ist bereits aktuell."));
    }
    let installer = update::download(&cfg, &feed, on_progress)
        .await
        .map_err(err)?;
    update::start_installer(&installer).map_err(err)?;
    Ok(feed.version)
}

// ---------- Server-Konfiguration ----------

/// Holt Packs, News und Server vom eigenen Server. Fehler (offline, Server down) sind
/// unkritisch – dann gilt der zuletzt geladene Stand weiter.
pub async fn refresh_remote_config() -> bool {
    config::refresh_remote_config().await.unwrap_or(false)
}

// ---------- News ----------

pub async fn news() -> Vec<Value> {
    let cfg = config::load_launcher_config();
    if cfg.news_url.is_empty() {
        return cfg.news;
    }
    let fetched = async {
        let v: Value = serde_json::from_str(&modpack::fetch_source(&cfg.news_url).await?)?;
        Ok::<_, anyhow::Error>(match v {
            Value::Array(items) => items,
            other => other["items"].as_array().cloned().unwrap_or_default(),
        })
    };
    fetched.await.unwrap_or(cfg.news)
}


// ---------- Einstellungen übernehmen ----------

/// Optionen, Ressourcenpakete usw. aus einer anderen Instanz kopieren.
pub async fn copy_settings(from: String, to: String, parts: Parts, worlds: WorldTargets) -> Res<Summary> {
    tokio::task::spawn_blocking(move || transfer::copy_between(&current_prism(), &from, &to, parts, &worlds))
        .await
        .map_err(err)?
        .map_err(err)
}
