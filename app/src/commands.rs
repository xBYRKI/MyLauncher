//! Tauri-Befehle für die Oberfläche. Die eigentliche Logik steckt in `launcher-core`.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use launcher_core::config::{self, Settings};
use launcher_core::modpack::{self, PackStatus, SyncResult};
use launcher_core::ping::{self, ServerStatus};
use launcher_core::prism::{self, Account, Instance, Prism};
use launcher_core::waypoints::{self, Health, SyncOptions};
use launcher_core::xaero;
use notify::Watcher;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    // anyhow mit Ursachenkette, ohne Debug-Rauschen
    format!("{e:#}")
}

#[derive(Default)]
struct AppState {
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    busy_packs: Mutex<HashSet<String>>,
}

fn current_prism() -> Prism {
    prism::resolve(&config::load_settings())
}

fn hidden() -> Vec<String> {
    config::load_launcher_config().hidden_instances
}

/// Instanzordner beobachten und die UI bei Änderungen benachrichtigen.
fn rewatch(app: &AppHandle) {
    let state = app.state::<AppState>();
    let handle = app.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = handle.emit("instances-changed", ());
        }
    })
    .ok()
    .and_then(|mut w| {
        w.watch(
            &current_prism().instances_dir,
            notify::RecursiveMode::NonRecursive,
        )
        .ok()
        .map(|_| w)
    });
    *state.watcher.lock().unwrap() = watcher;
}

fn after_launch(app: &AppHandle) {
    match config::load_settings().after_launch.as_str() {
        "close" => app.exit(0),
        "minimize" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.minimize();
            }
        }
        _ => {}
    }
}

fn ensure_configured_pack(source: &str) -> CmdResult<()> {
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

#[tauri::command]
fn app_info(app: AppHandle) -> Value {
    let mut v = serde_json::to_value(config::load_launcher_config()).unwrap_or_default();
    v["appVersion"] = json!(app.package_info().version.to_string());
    v
}

#[tauri::command]
fn get_settings() -> Settings {
    config::load_settings()
}

#[tauri::command]
fn set_settings(app: AppHandle, patch: Value) -> CmdResult<Settings> {
    let settings = config::patch_settings(&patch).map_err(err)?;
    rewatch(&app);
    Ok(settings)
}

#[tauri::command]
fn open_external(url: String) -> CmdResult<()> {
    if url.starts_with("https://") || url.starts_with("http://") {
        opener::open_browser(&url).map_err(err)
    } else {
        Err("Nur http(s)-Links".into())
    }
}

#[tauri::command]
async fn pick_file() -> Option<String> {
    let mut dialog = rfd::AsyncFileDialog::new().set_title("Prism Launcher auswählen");
    if cfg!(windows) {
        dialog = dialog.add_filter("Prism Launcher", &["exe"]);
    }
    dialog
        .pick_file()
        .await
        .map(|f| f.path().to_string_lossy().into_owned())
}

#[tauri::command]
async fn pick_dir() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .pick_folder()
        .await
        .map(|f| f.path().to_string_lossy().into_owned())
}

// ---------- Prism ----------

#[tauri::command]
fn prism_status() -> Prism {
    current_prism()
}

#[tauri::command]
async fn instances() -> Vec<Instance> {
    tokio::task::spawn_blocking(|| prism::list_instances(&current_prism(), &hidden()))
        .await
        .unwrap_or_default()
}

#[tauri::command]
fn accounts() -> Vec<Account> {
    prism::list_accounts(&current_prism())
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

#[tauri::command]
async fn launch(app: AppHandle, instance_id: String, server: Option<String>) -> CmdResult<()> {
    let settings = config::load_settings();
    let p = current_prism();
    pull_waypoints_if_needed(&p, &instance_id).await;
    let server = if settings.join_server {
        server.unwrap_or_default()
    } else {
        String::new()
    };
    prism::launch(&p, &instance_id, &settings.account, &server).map_err(err)?;
    after_launch(&app);
    Ok(())
}

#[tauri::command]
fn show_in_prism(instance_id: Option<String>) -> CmdResult<()> {
    prism::show(&current_prism(), instance_id.as_deref()).map_err(err)
}

#[tauri::command]
fn open_folder(instance_id: Option<String>) -> CmdResult<()> {
    let p = current_prism();
    let target = match instance_id {
        Some(id) => prism::instance_dir(&p, &id).map_err(err)?,
        None => p.data_dir,
    };
    opener::open(&target).map_err(err)
}

// ---------- Modpacks ----------

#[tauri::command]
async fn packs() -> Vec<PackStatus> {
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

async fn do_sync_pack(app: &AppHandle, source: &str) -> CmdResult<SyncResult> {
    ensure_configured_pack(source)?;
    let state = app.state::<AppState>();
    if !state.busy_packs.lock().unwrap().insert(source.to_string()) {
        return Err("Dieses Modpack wird bereits aktualisiert".into());
    }
    let brand = config::load_launcher_config().brand.name;
    let emitter = app.clone();
    let src = source.to_string();
    let result = modpack::sync_pack(&current_prism(), source, &brand, move |p| {
        let mut payload = serde_json::to_value(&p).unwrap_or_default();
        payload["source"] = json!(src);
        let _ = emitter.emit("pack-progress", payload);
    })
    .await
    .map_err(err);
    state.busy_packs.lock().unwrap().remove(source);
    result
}

#[tauri::command]
async fn sync_pack(app: AppHandle, source: String) -> CmdResult<SyncResult> {
    do_sync_pack(&app, &source).await
}

#[tauri::command]
async fn play_pack(app: AppHandle, source: String) -> CmdResult<SyncResult> {
    let settings = config::load_settings();
    let p = current_prism();
    let result = if settings.auto_update_packs {
        do_sync_pack(&app, &source).await?
    } else {
        ensure_configured_pack(&source)?;
        let (m, _) = modpack::load_manifest(&source).await.map_err(err)?;
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
    after_launch(&app);
    Ok(result)
}

// ---------- News & Server ----------

#[tauri::command]
async fn news() -> Vec<Value> {
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

#[tauri::command]
async fn server_status(address: String) -> ServerStatus {
    ping::status(&address).await
}

// ---------- Wegpunkte ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorldInstance {
    id: String,
    name: String,
    waypoints: usize,
    shared: usize,
    last_sync: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorldEntry {
    folder: String,
    display: String,
    multiplayer: bool,
    enabled: bool,
    instances: Vec<WorldInstance>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookInstance {
    id: String,
    name: String,
    hooks: bool,
    has_xaero: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WaypointOverview {
    worlds: Vec<WorldEntry>,
    instances: Vec<HookInstance>,
    prism_running: bool,
    server_url: String,
}

#[tauri::command]
async fn waypoint_overview() -> WaypointOverview {
    tokio::task::spawn_blocking(|| {
        let settings = config::load_settings();
        let cfg = config::load_launcher_config();
        let p = prism::resolve(&settings);
        let ws = &settings.waypoints;
        let mut worlds: Vec<WorldEntry> = vec![];
        let mut hook_list = vec![];
        for (inst, game_dir) in waypoints::instance_game_dirs(&p, &cfg.hidden_instances) {
            let found = xaero::list_worlds(&game_dir, |w| {
                waypoints::is_shared(w, &ws.shared_set, ws.share_all)
            });
            hook_list.push(HookInstance {
                id: inst.id.clone(),
                name: inst.name.clone(),
                hooks: inst.waypoint_hooks,
                has_xaero: !found.is_empty() || xaero::minimap_root(&game_dir).exists(),
            });
            for w in found {
                let entry = match worlds.iter_mut().position(|e| e.folder == w.folder) {
                    Some(i) => &mut worlds[i],
                    None => {
                        worlds.push(WorldEntry {
                            enabled: ws.worlds.contains(&w.folder),
                            folder: w.folder.clone(),
                            display: w.display.clone(),
                            multiplayer: w.multiplayer,
                            instances: vec![],
                        });
                        worlds.last_mut().unwrap()
                    }
                };
                entry.instances.push(WorldInstance {
                    last_sync: waypoints::last_synced(&game_dir, &w.folder),
                    id: inst.id.clone(),
                    name: inst.name.clone(),
                    waypoints: w.waypoints,
                    shared: w.shared,
                });
            }
        }
        worlds.sort_by(|a, b| {
            b.enabled
                .cmp(&a.enabled)
                .then(b.multiplayer.cmp(&a.multiplayer))
                .then(a.display.cmp(&b.display))
        });
        hook_list.sort_by(|a, b| b.has_xaero.cmp(&a.has_xaero).then(a.name.cmp(&b.name)));
        WaypointOverview {
            worlds,
            instances: hook_list,
            prism_running: prism::is_prism_running(),
            server_url: config::waypoint_server_url(&settings, &cfg),
        }
    })
    .await
    .expect("Übersicht")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncLine {
    instance: String,
    ok: bool,
    message: String,
}

/// Jetzt synchronisieren – für eine Instanz oder alle. Laufende Spiele werden übersprungen,
/// weil Xaero die Dateien beim Beenden sonst wieder überschreibt.
#[tauri::command]
async fn waypoint_sync(instance_id: Option<String>) -> CmdResult<Vec<SyncLine>> {
    let settings = config::load_settings();
    let cfg = config::load_launcher_config();
    let p = prism::resolve(&settings);
    if settings.waypoints.worlds.is_empty() {
        return Err("Keine Welt für den Sync freigegeben.".into());
    }
    let opts = SyncOptions::new(&settings, &cfg, prism::player_name(&p, &settings)).map_err(err)?;
    let mut lines = vec![];
    for (inst, game_dir) in waypoints::instance_game_dirs(&p, &cfg.hidden_instances) {
        if instance_id.as_ref().is_some_and(|id| id != &inst.id) {
            continue;
        }
        let root = xaero::minimap_root(&game_dir);
        if !settings
            .waypoints
            .worlds
            .iter()
            .any(|w| root.join(w).is_dir())
        {
            continue;
        }
        let inst_dir = p.instances_dir.join(&inst.id);
        if prism::is_instance_running(&inst_dir) {
            lines.push(SyncLine {
                instance: inst.name.clone(),
                ok: false,
                message: "Spiel läuft – Sync nach dem Beenden".into(),
            });
            continue;
        }
        for r in waypoints::sync_game_dir(&game_dir, &settings.waypoints.worlds, &opts).await {
            lines.push(match r {
                Ok(r) => SyncLine {
                    instance: inst.name.clone(),
                    ok: true,
                    message: format!(
                        "{}: {} hochgeladen, {} übernommen, {} geteilt",
                        xaero::display_name(&r.folder),
                        r.pushed_added + r.pushed_deleted,
                        r.pulled_added + r.pulled_removed,
                        r.total
                    ),
                },
                Err(e) => SyncLine {
                    instance: inst.name.clone(),
                    ok: false,
                    message: err(e),
                },
            });
        }
    }
    Ok(lines)
}

#[tauri::command]
fn waypoint_set_hooks(instance_id: String, enabled: bool) -> CmdResult<()> {
    if prism::is_prism_running() {
        return Err(
            "Bitte Prism Launcher zuerst schließen – sonst überschreibt Prism die Änderung.".into(),
        );
    }
    let dir = prism::instance_dir(&current_prism(), &instance_id).map_err(err)?;
    let exe = std::env::current_exe().map_err(err)?;
    waypoints::set_hooks(&dir, &exe, enabled).map_err(err)
}

#[tauri::command]
async fn waypoint_test() -> CmdResult<Health> {
    let settings = config::load_settings();
    let cfg = config::load_launcher_config();
    let opts = SyncOptions::new(&settings, &cfg, String::new()).map_err(err)?;
    waypoints::test_connection(&opts).await.map_err(err)
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            rewatch(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_settings,
            set_settings,
            open_external,
            pick_file,
            pick_dir,
            prism_status,
            instances,
            accounts,
            launch,
            show_in_prism,
            open_folder,
            packs,
            sync_pack,
            play_pack,
            news,
            server_status,
            waypoint_overview,
            waypoint_sync,
            waypoint_set_hooks,
            waypoint_test,
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der App");
}
