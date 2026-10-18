//! Xaero-Wegpunkte mit Freunden teilen – über den eigenen Sync-Server (`crates/server`).
//!
//! Ablauf pro Welt:
//! 1. Lokale geteilte Wegpunkte lesen (Set "Team" bzw. alle, ohne Todespunkte).
//! 2. Mit dem Stand des letzten Syncs (`mylauncher-sync.json` im Weltordner) vergleichen:
//!    neu = lokal, aber nicht im letzten Stand; gelöscht = im letzten Stand, lokal weg.
//!    Eine Änderung (Name, Position, Farbe) ist Löschen + Neu.
//! 3. Änderungen an den Server schicken, aktuellen Raum-Stand zurückbekommen.
//! 4. Geteilte Wegpunkte in den Xaero-Dateien durch den Server-Stand ersetzen.
//!
//! Xaero schreibt seine Dateien selbst, solange das Spiel läuft. Deshalb läuft der Sync
//! über Prism-Hooks: vor dem Start (`PreLaunchCommand`) und nach dem Beenden (`PostExitCommand`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use protocol::{RoomState, SharedWaypoint, SyncRequest, WaypointData};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{self, LauncherConfig, Settings};
use crate::ini::Ini;
use crate::prism::{self, Prism};
use crate::xaero::{self, Waypoint, WaypointFile, DEATH_TYPE, DEFAULT_SET};

pub const SNAPSHOT_FILE: &str = "mylauncher-sync.json";
pub const HOOK_FLAG: &str = "--xaero-sync";

#[derive(Clone, Debug)]
pub struct SyncOptions {
    pub server_url: String,
    pub token: String,
    pub player: String,
    pub shared_set: String,
    pub share_all: bool,
}

impl SyncOptions {
    pub fn new(settings: &Settings, cfg: &LauncherConfig, player: String) -> Result<Self> {
        let server_url = config::waypoint_server_url(settings, cfg);
        if server_url.is_empty() {
            bail!("Kein Wegpunkt-Server eingetragen (Einstellungen → Wegpunkte).");
        }
        if settings.waypoints.token.trim().is_empty() {
            bail!("Kein Team-Schlüssel eingetragen (Einstellungen → Wegpunkte).");
        }
        let shared_set = settings.waypoints.shared_set.trim();
        Ok(Self {
            server_url,
            token: settings.waypoints.token.trim().to_string(),
            player,
            shared_set: if shared_set.is_empty() {
                "Team".into()
            } else {
                shared_set.into()
            },
            share_all: settings.waypoints.share_all,
        })
    }

    pub fn is_shared(&self, w: &Waypoint) -> bool {
        is_shared(w, &self.shared_set, self.share_all)
    }

    fn api(&self, segments: &[&str]) -> Result<Url> {
        let mut url = Url::parse(&self.server_url).context("Ungültige Server-URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow!("Ungültige Server-URL"))?
            .pop_if_empty()
            .push("api")
            .extend(segments);
        Ok(url)
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()
            .expect("HTTP-Client")
    }
}

pub fn is_shared(w: &Waypoint, shared_set: &str, share_all: bool) -> bool {
    w.kind != DEATH_TYPE && (share_all || w.set == shared_set)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    rev: u64,
    synced_at: u64,
    waypoints: Vec<SharedWaypoint>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldSyncReport {
    pub folder: String,
    pub pushed_added: usize,
    pub pushed_deleted: usize,
    pub pulled_added: usize,
    pub pulled_removed: usize,
    pub total: usize,
    pub rev: u64,
}

fn to_data(dim: &str, world: &str, w: &Waypoint) -> WaypointData {
    WaypointData {
        dim: dim.into(),
        world: world.into(),
        name: w.name.clone(),
        initials: w.initials.clone(),
        x: w.x,
        y: w.y.clone(),
        z: w.z,
        color: w.color,
        extra: w.extra.clone(),
    }
}

fn from_shared(d: &WaypointData, set: &str, disabled: bool) -> Waypoint {
    Waypoint {
        name: d.name.clone(),
        initials: d.initials.clone(),
        x: d.x,
        y: d.y.clone(),
        z: d.z,
        color: d.color,
        disabled,
        kind: 0,
        set: set.into(),
        extra: d.extra.clone(),
    }
}

/// Erlaubt nur `dim%…`-Ordner und einfache Dateinamen vom Server.
fn safe_component(s: &str) -> bool {
    protocol::valid_room(s) && !s.contains('/') && !s.contains('\\')
}

/// Letzter Sync-Zeitpunkt einer Welt (für die UI).
pub fn last_synced(game_dir: &Path, folder: &str) -> Option<u64> {
    let path = xaero::minimap_root(game_dir)
        .join(folder)
        .join(SNAPSHOT_FILE);
    let snap: Snapshot = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(snap.synced_at)
}

pub async fn sync_world(
    game_dir: &Path,
    folder: &str,
    opts: &SyncOptions,
) -> Result<WorldSyncReport> {
    if !protocol::valid_room(folder) {
        bail!("Ungültiger Weltname: {folder}");
    }
    let world_dir = xaero::minimap_root(game_dir).join(folder);
    let files = xaero::world_files(&world_dir);
    let snapshot_path = world_dir.join(SNAPSHOT_FILE);

    // 1) Lokaler Stand
    let mut local: Vec<WaypointData> = vec![];
    let mut disabled: HashMap<String, bool> = HashMap::new();
    for (dim, world, path) in &files {
        if let Some(file) = WaypointFile::load(path) {
            for w in file.waypoints().filter(|w| opts.is_shared(w)) {
                let data = to_data(dim, world, w);
                disabled.insert(data.key(), w.disabled);
                local.push(data);
            }
        }
    }
    let local_keys: HashSet<String> = local.iter().map(WaypointData::key).collect();

    // 2) Unterschiede zum letzten Sync
    let snapshot: Snapshot = std::fs::read_to_string(&snapshot_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let snap_keys: HashSet<String> = snapshot.waypoints.iter().map(|w| w.data.key()).collect();
    let deleted: Vec<String> = snapshot
        .waypoints
        .iter()
        .filter(|w| !local_keys.contains(&w.data.key()))
        .map(|w| w.id.clone())
        .collect();
    let mut seen = HashSet::new();
    let added: Vec<WaypointData> = local
        .iter()
        .filter(|d| !snap_keys.contains(&d.key()) && seen.insert(d.key()))
        .cloned()
        .collect();

    // 3) Server
    let request = SyncRequest {
        player: opts.player.clone(),
        added: added.clone(),
        deleted: deleted.clone(),
    };
    let res = opts
        .client()
        .post(opts.api(&["rooms", folder, "sync"])?)
        .bearer_auth(&opts.token)
        .json(&request)
        .send()
        .await
        .context("Wegpunkt-Server nicht erreichbar")?;
    match res.status().as_u16() {
        200 => {}
        401 | 403 => bail!("Team-Schlüssel wird vom Server abgelehnt"),
        code => bail!("Wegpunkt-Server antwortet mit HTTP {code}"),
    }
    let state: RoomState = res
        .json()
        .await
        .context("Ungültige Antwort vom Wegpunkt-Server")?;

    // 4) Lokal anwenden
    let set = if opts.share_all {
        DEFAULT_SET
    } else {
        opts.shared_set.as_str()
    };
    let mut by_file: BTreeMap<(String, String), Vec<&SharedWaypoint>> = BTreeMap::new();
    for w in &state.waypoints {
        if safe_component(&w.data.dim)
            && w.data.dim.starts_with("dim%")
            && safe_component(&w.data.world)
        {
            by_file
                .entry((w.data.dim.clone(), w.data.world.clone()))
                .or_default()
                .push(w);
        }
    }
    let server_keys: HashSet<String> = state.waypoints.iter().map(|w| w.data.key()).collect();

    let mut targets: Vec<(String, String, PathBuf)> = files.clone();
    for (dim, world) in by_file.keys() {
        if !targets.iter().any(|(d, w, _)| d == dim && w == world) {
            targets.push((
                dim.clone(),
                world.clone(),
                world_dir.join(dim).join(format!("{world}.txt")),
            ));
        }
    }
    for (dim, world, path) in &targets {
        let original = WaypointFile::load(path);
        let mut file = original.clone().unwrap_or_else(WaypointFile::new_empty);
        file.retain(|w| !opts.is_shared(w));
        let incoming = by_file.get(&(dim.clone(), world.clone()));
        if let Some(list) = incoming {
            if !opts.share_all {
                file.ensure_set(set);
            }
            for w in list {
                let off = disabled.get(&w.data.key()).copied().unwrap_or(false);
                file.push(from_shared(&w.data, set, off));
            }
        }
        let rendered = file.render();
        if original.map(|o| o.render()) != Some(rendered.clone()) {
            if incoming.is_none() && !path.exists() {
                continue;
            }
            std::fs::create_dir_all(path.parent().unwrap_or(&world_dir))?;
            std::fs::write(path, rendered)
                .with_context(|| format!("{} nicht schreibbar", path.display()))?;
        }
    }

    let snap = Snapshot {
        rev: state.rev,
        synced_at: crate::now_millis(),
        waypoints: state.waypoints.clone(),
    };
    std::fs::create_dir_all(&world_dir)?;
    std::fs::write(&snapshot_path, serde_json::to_string_pretty(&snap)?)?;

    Ok(WorldSyncReport {
        folder: folder.into(),
        pushed_added: added.len(),
        pushed_deleted: deleted.len(),
        pulled_added: server_keys.difference(&local_keys).count(),
        pulled_removed: local_keys.difference(&server_keys).count(),
        total: state.waypoints.len(),
        rev: state.rev,
    })
}

/// Alle freigegebenen Welten eines Spielordners synchronisieren.
/// Welten, die es dort noch nicht gibt, werden angelegt, sobald Xaero sie einmal erstellt hat.
pub async fn sync_game_dir(
    game_dir: &Path,
    worlds: &[String],
    opts: &SyncOptions,
) -> Vec<Result<WorldSyncReport>> {
    let root = xaero::minimap_root(game_dir);
    let mut out = vec![];
    for folder in worlds {
        if root.join(folder).is_dir() {
            out.push(
                sync_world(game_dir, folder, opts)
                    .await
                    .map_err(|e| anyhow!("{folder}: {e:#}")),
            );
        }
    }
    out
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    #[serde(default)]
    pub team: String,
}

pub async fn test_connection(opts: &SyncOptions) -> Result<Health> {
    let res = opts
        .client()
        .get(opts.api(&["health"])?)
        .bearer_auth(&opts.token)
        .send()
        .await
        .context("Server nicht erreichbar")?;
    match res.status().as_u16() {
        200 => Ok(res.json().await?),
        401 | 403 => bail!("Team-Schlüssel wird abgelehnt"),
        code => bail!("HTTP {code}"),
    }
}

// ---------- Prism-Hooks ----------

pub fn hooks_installed(cfg: &Ini) -> bool {
    cfg.general("PreLaunchCommand")
        .is_some_and(|c| c.contains(HOOK_FLAG))
}

/// Trägt den Launcher als Pre-Launch-/Post-Exit-Befehl in die Instanz ein.
/// Prism muss dafür geschlossen sein, sonst überschreibt es `instance.cfg` mit seinem Stand.
pub fn set_hooks(instance_dir: &Path, exe: &Path, enable: bool) -> Result<()> {
    let path = instance_dir.join("instance.cfg");
    let mut ini = Ini::load(&path);
    let ours = hooks_installed(&ini);
    if enable {
        for key in ["PreLaunchCommand", "PostExitCommand"] {
            let existing = ini.general(key).unwrap_or_default();
            if !existing.trim().is_empty() && !existing.contains(HOOK_FLAG) {
                bail!("Die Instanz hat bereits einen eigenen {key} – bitte in Prism entfernen.");
            }
        }
        let exe = exe.to_string_lossy();
        ini.set("General", "OverrideCommands", "true");
        ini.set(
            "General",
            "PreLaunchCommand",
            &format!("\"{exe}\" {HOOK_FLAG} pre \"$INST_MC_DIR\""),
        );
        ini.set(
            "General",
            "PostExitCommand",
            &format!("\"{exe}\" {HOOK_FLAG} post \"$INST_MC_DIR\""),
        );
    } else if ours {
        ini.set("General", "OverrideCommands", "false");
        ini.set("General", "PreLaunchCommand", "");
        ini.set("General", "PostExitCommand", "");
    } else {
        return Ok(());
    }
    ini.save(&path)
        .with_context(|| format!("{} nicht schreibbar", path.display()))
}

/// Einstieg für den Hook-Modus: `mylauncher --xaero-sync <pre|post> <INST_MC_DIR>`.
/// Gibt Log-Zeilen zurück; Fehler dürfen den Spielstart nie verhindern.
pub async fn run_hook(phase: &str, game_dir: &Path) -> Vec<String> {
    let settings = config::load_settings();
    let cfg = config::load_launcher_config();
    let prism = prism::resolve(&settings);
    let mut log = vec![format!("[{phase}] {}", game_dir.display())];
    if settings.waypoints.worlds.is_empty() {
        log.push("Keine Welten für den Sync freigegeben.".into());
        return log;
    }
    let opts = match SyncOptions::new(&settings, &cfg, prism::player_name(&prism, &settings)) {
        Ok(o) => o,
        Err(e) => {
            log.push(format!("Übersprungen: {e}"));
            return log;
        }
    };
    for result in sync_game_dir(game_dir, &settings.waypoints.worlds, &opts).await {
        log.push(match result {
            Ok(r) => format!(
                "{}: +{} −{} hochgeladen, +{} −{} übernommen, {} geteilt (rev {})",
                r.folder,
                r.pushed_added,
                r.pushed_deleted,
                r.pulled_added,
                r.pulled_removed,
                r.total,
                r.rev
            ),
            Err(e) => format!("Fehler: {e:#}"),
        });
    }
    log
}

/// Spielordner aller Instanzen (für Weltübersicht und "Alle synchronisieren").
pub fn instance_game_dirs(prism: &Prism, hidden: &[String]) -> Vec<(prism::Instance, PathBuf)> {
    prism::list_instances(prism, hidden)
        .into_iter()
        .map(|i| {
            let dir = prism::game_dir(&prism.instances_dir.join(&i.id));
            (i, dir)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_roundtrip_and_foreign_commands() {
        let dir = std::env::temp_dir().join(format!("mylauncher-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("instance.cfg");
        std::fs::write(
            &cfg,
            "[General]\nname=Test\nPreLaunchCommand=\n\n[UI]\nmods_Page\\Columns=\"AA==\"\n",
        )
        .unwrap();
        let exe = Path::new(r"C:\Program Files\My Launcher\mylauncher.exe");

        set_hooks(&dir, exe, true).unwrap();
        let ini = Ini::load(&cfg);
        assert!(hooks_installed(&ini));
        assert_eq!(
            ini.general("PreLaunchCommand").unwrap(),
            format!("\"{}\" {HOOK_FLAG} pre \"$INST_MC_DIR\"", exe.display())
        );
        assert_eq!(ini.get("UI", "mods_Page\\Columns").unwrap(), "AA==");

        set_hooks(&dir, exe, false).unwrap();
        let ini = Ini::load(&cfg);
        assert!(!hooks_installed(&ini));
        assert_eq!(ini.general("OverrideCommands").unwrap(), "false");

        // Fremde Befehle werden nicht überschrieben
        std::fs::write(&cfg, "[General]\nPreLaunchCommand=echo hallo\n").unwrap();
        assert!(set_hooks(&dir, exe, true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
