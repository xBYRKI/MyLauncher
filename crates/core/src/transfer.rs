//! Spieleinstellungen aus einer anderen Prism-Instanz übernehmen: Optionen und Tastenbelegung,
//! Ressourcen- und Shaderpakete, Serverliste, Xaero-Wegpunkte und -Weltkarte. Damit muss
//! niemand im neuen Modpack alles wieder von Hand einstellen.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::prism::{self, Prism};
use crate::xaero::{self, WaypointFile};

/// Was übernommen werden soll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parts {
    /// `options.txt` (Grafik, Ton, Tastenbelegung, aktive Ressourcenpakete) und `optionsof.txt`
    pub options: bool,
    pub resource_packs: bool,
    /// `shaderpacks/` samt gewähltem Shader (Iris/OptiFine)
    pub shader_packs: bool,
    pub servers: bool,
    /// Xaero-Wegpunkte; werden mit vorhandenen zusammengeführt
    pub waypoints: bool,
    /// Bereits erkundete Kacheln der Xaero-Weltkarte
    pub world_map: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Self {
            options: true,
            resource_packs: true,
            shader_packs: true,
            servers: false,
            waypoints: true,
            world_map: true,
        }
    }
}

impl Parts {
    pub fn any(&self) -> bool {
        self.options || self.resource_packs || self.shader_packs || self.servers || self.xaero()
    }

    pub fn xaero(&self) -> bool {
        self.waypoints || self.world_map
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Übernommene Einstellungsdateien (options.txt, servers.dat …)
    pub files: usize,
    /// Neu kopierte Ressourcen- und Shaderpakete
    pub packs: usize,
    /// Pakete, die im Ziel schon lagen und unangetastet blieben
    pub skipped: usize,
    /// Neu übernommene Wegpunkte
    pub waypoints: usize,
    /// Neu kopierte Dateien der Weltkarte
    pub map_files: usize,
}

/// Ziel je Xaero-Weltordner der Quelle: `Some(ordner)` = dorthin, `None` = auslassen.
/// Fehlt ein Ordner, landet er unter gleichem Namen. Nötig, wenn derselbe Server in den
/// Instanzen unter verschiedenen Adressen eingetragen ist – Xaero legt pro Adresse einen
/// eigenen Ordner an.
pub type WorldTargets = BTreeMap<String, Option<String>>;

/// Weltordner einer Instanz, für die Xaero Wegpunkte oder Karten hat.
pub fn xaero_worlds(prism: &Prism, id: &str) -> Vec<String> {
    prism::instance_dir(prism, id)
        .map(|dir| xaero::world_folders(&prism::game_dir(&dir)))
        .unwrap_or_default()
}

/// Vorschlag: Gibt es auf beiden Seiten genau einen Mehrspieler-Ordner ohne Gegenstück,
/// ist es vermutlich derselbe Server unter anderer Adresse.
pub fn suggest_targets(source: &[String], target: &[String]) -> WorldTargets {
    let unmatched = |list: &[String], other: &[String]| -> Vec<String> {
        list.iter()
            .filter(|w| w.starts_with("Multiplayer_") && !other.contains(w))
            .cloned()
            .collect()
    };
    let mut out = WorldTargets::new();
    if let ([from], [to]) = (
        unmatched(source, target).as_slice(),
        unmatched(target, source).as_slice(),
    ) {
        out.insert(from.clone(), Some(to.clone()));
    }
    out
}

/// Dateien im Format `schlüssel:wert`, die zusammengeführt statt ersetzt werden.
const OPTION_FILES: &[&str] = &["options.txt", "optionsof.txt"];
/// Auswahl des Shaders: Iris bzw. OptiFine.
const SHADER_FILES: &[&str] = &["config/iris.properties", "optionsshaders.txt"];

/// Übernimmt die gewählten Teile von Instanz `from` nach `to`.
pub fn copy_between(
    prism: &Prism,
    from: &str,
    to: &str,
    parts: Parts,
    worlds: &WorldTargets,
) -> Result<Summary> {
    let from_dir = prism::instance_dir(prism, from)?;
    let to_dir = prism::instance_dir(prism, to)?;
    if from_dir == to_dir {
        bail!("Quelle und Ziel sind dieselbe Instanz");
    }
    // Minecraft schreibt options.txt beim Beenden zurück und würde alles wieder überschreiben.
    if prism::is_instance_running(&to_dir) {
        bail!("Die Instanz läuft gerade – bitte erst Minecraft schließen");
    }
    copy_game_dir(
        &prism::game_dir(&from_dir),
        &prism::game_dir(&to_dir),
        parts,
        worlds,
    )
}

pub fn copy_game_dir(
    from: &Path,
    to: &Path,
    parts: Parts,
    worlds: &WorldTargets,
) -> Result<Summary> {
    if !parts.any() {
        bail!("Nichts zum Übernehmen ausgewählt");
    }
    if !from.is_dir() {
        bail!("Die Quell-Instanz wurde noch nie gestartet – dort gibt es keine Einstellungen");
    }
    std::fs::create_dir_all(to).with_context(|| format!("{} nicht anlegbar", to.display()))?;
    let mut sum = Summary::default();
    if parts.options {
        for name in OPTION_FILES {
            if merge_file(&from.join(name), &to.join(name))? {
                sum.files += 1;
            }
        }
    }
    if parts.resource_packs {
        copy_packs(
            &from.join("resourcepacks"),
            &to.join("resourcepacks"),
            &mut sum,
        )?;
    }
    if parts.shader_packs {
        copy_packs(&from.join("shaderpacks"), &to.join("shaderpacks"), &mut sum)?;
        for name in SHADER_FILES {
            if replace_file(&from.join(name), &to.join(name))? {
                sum.files += 1;
            }
        }
    }
    if parts.servers && replace_file(&from.join("servers.dat"), &to.join("servers.dat"))? {
        sum.files += 1;
    }
    if parts.xaero() {
        copy_xaero(from, to, parts, worlds, &mut sum)?;
    }
    Ok(sum)
}

fn copy_xaero(
    from: &Path,
    to: &Path,
    parts: Parts,
    worlds: &WorldTargets,
    sum: &mut Summary,
) -> Result<()> {
    let (wp_from, wp_to) = (xaero::minimap_root(from), xaero::minimap_root(to));
    let (map_from, map_to) = (xaero::world_map_root(from), xaero::world_map_root(to));
    for world in xaero::world_folders(from) {
        let Some(target) = worlds.get(&world).cloned().unwrap_or(Some(world.clone())) else {
            continue;
        };
        if target.is_empty() || target.contains(['/', '\\']) || target.starts_with('.') {
            bail!("Ungültiger Zielordner: {target}");
        }
        if parts.waypoints {
            sum.waypoints += copy_waypoints(&wp_from.join(&world), &wp_to.join(&target))?;
        }
        if parts.world_map {
            sum.map_files += copy_missing(&map_from.join(&world), &map_to.join(&target))
                .with_context(|| format!("Weltkarte von {world} nicht kopierbar"))?;
        }
    }
    Ok(())
}

/// Wegpunktdateien (`dim%…/*.txt`) einer Welt zusammenführen. Die Sync-Datei des Launchers
/// bleibt bewusst zurück – sie beschreibt den Stand der Quell-Instanz.
fn copy_waypoints(from: &Path, to: &Path) -> Result<usize> {
    let files = xaero::world_files(from);
    if files.is_empty() {
        return Ok(0);
    }
    let config = to.join("config.txt");
    if !config.exists() {
        replace_file(&from.join("config.txt"), &config)?;
    }
    let mut added = 0;
    for (dim, stem, path) in files {
        let Some(source) = WaypointFile::load(&path) else {
            continue;
        };
        let dest = to.join(&dim).join(format!("{stem}.txt"));
        let mut file = WaypointFile::load(&dest).unwrap_or_else(WaypointFile::new_empty);
        let n = file.merge_from(&source);
        if n == 0 {
            continue;
        }
        backup(&dest)?;
        std::fs::create_dir_all(to.join(&dim))?;
        std::fs::write(&dest, file.render())
            .with_context(|| format!("{} nicht schreibbar", dest.display()))?;
        added += n;
    }
    Ok(added)
}

/// Alles kopieren, was im Ziel fehlt; vorhandene Dateien (z. B. neuere Kartenkacheln) bleiben.
fn copy_missing(from: &Path, to: &Path) -> std::io::Result<usize> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        let mut n = 0;
        for entry in std::fs::read_dir(from)?.flatten() {
            n += copy_missing(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(n)
    } else if from.is_file() && !to.exists() {
        std::fs::copy(from, to).map(|_| 1)
    } else {
        Ok(0)
    }
}

/// Vor dem Überschreiben eine Sicherung `<datei>.bak` anlegen.
fn backup(path: &Path) -> Result<()> {
    if path.is_file() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, &bak)
            .with_context(|| format!("Sicherung von {} fehlgeschlagen", path.display()))?;
    }
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> Result<bool> {
    if !from.is_file() {
        return Ok(false);
    }
    backup(to)?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).with_context(|| format!("{} nicht kopierbar", from.display()))?;
    Ok(true)
}

/// Werte der Quelle gewinnen; Schlüssel, die nur das Ziel kennt (z. B. Tasten von Mods,
/// die es nur im Modpack gibt), bleiben erhalten.
fn merge_file(from: &Path, to: &Path) -> Result<bool> {
    let Ok(source) = std::fs::read_to_string(from) else {
        return Ok(false);
    };
    let merged = match std::fs::read_to_string(to) {
        Ok(target) => merge_options(&target, &source),
        Err(_) => source,
    };
    backup(to)?;
    std::fs::write(to, merged).with_context(|| format!("{} nicht schreibbar", to.display()))?;
    Ok(true)
}

fn option_key(line: &str) -> Option<&str> {
    line.split_once(':')
        .map(|(k, _)| k)
        .filter(|k| !k.is_empty())
}

pub fn merge_options(target: &str, source: &str) -> String {
    let mut from_source: HashMap<&str, &str> = HashMap::new();
    let mut source_order = vec![];
    for line in source.lines() {
        if let Some(key) = option_key(line) {
            if from_source.insert(key, line).is_none() {
                source_order.push(key);
            }
        }
    }
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for line in target.lines() {
        match option_key(line) {
            Some(key) => {
                seen.insert(key);
                out.push(*from_source.get(key).unwrap_or(&line));
            }
            None if !line.trim().is_empty() => out.push(line),
            None => {}
        }
    }
    out.extend(
        source_order
            .iter()
            .filter(|k| !seen.contains(*k))
            .map(|k| from_source[k]),
    );
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Pakete (Zip oder Ordner) kopieren, die im Ziel noch fehlen. Vorhandene bleiben, wie sie sind.
fn copy_packs(from: &Path, to: &Path, sum: &mut Summary) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    std::fs::create_dir_all(to).with_context(|| format!("{} nicht anlegbar", to.display()))?;
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        if target.exists() {
            sum.skipped += 1;
            continue;
        }
        copy_recursive(&entry.path(), &target)
            .with_context(|| format!("{} nicht kopierbar", entry.path().display()))?;
        sum.packs += 1;
    }
    Ok(())
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)?.flatten() {
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mylauncher-transfer-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn optionen_werden_zusammengefuehrt() {
        let target = "version:3700\nfov:0.0\nkey_key.modpack.map:key.keyboard.m\n";
        let source = "version:3700\nfov:0.5\nresourcePacks:[\"vanilla\",\"file/Faithful.zip\"]\n";
        assert_eq!(
            merge_options(target, source),
            "version:3700\nfov:0.5\nkey_key.modpack.map:key.keyboard.m\nresourcePacks:[\"vanilla\",\"file/Faithful.zip\"]\n"
        );
    }

    #[test]
    fn pakete_und_einstellungen_werden_uebernommen() {
        let root = tmp("copy");
        let (from, to) = (root.join("a"), root.join("b"));
        write(&from, "options.txt", "fov:0.5\n");
        write(&from, "resourcepacks/Faithful.zip", "neu");
        write(&from, "resourcepacks/Ordner/pack.mcmeta", "{}");
        write(&from, "resourcepacks/Schon da.zip", "quelle");
        write(&from, "servers.dat", "server");
        write(&to, "options.txt", "fov:0.0\n");
        write(&to, "resourcepacks/Schon da.zip", "ziel");

        let sum = copy_game_dir(&from, &to, Parts::default(), &WorldTargets::new()).unwrap();

        assert_eq!(
            std::fs::read_to_string(to.join("options.txt")).unwrap(),
            "fov:0.5\n"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("options.txt.bak")).unwrap(),
            "fov:0.0\n"
        );
        assert!(to.join("resourcepacks/Faithful.zip").is_file());
        assert!(to.join("resourcepacks/Ordner/pack.mcmeta").is_file());
        assert_eq!(
            std::fs::read_to_string(to.join("resourcepacks/Schon da.zip")).unwrap(),
            "ziel"
        );
        assert!(
            !to.join("servers.dat").exists(),
            "Serverliste ist nicht vorausgewählt"
        );
        assert_eq!((sum.files, sum.packs, sum.skipped), (1, 2, 1));
    }

    #[test]
    fn xaero_wird_zusammengefuehrt_und_umbenannt() {
        let root = tmp("xaero");
        let (from, to) = (root.join("a"), root.join("b"));
        let basis = "waypoint:Basis:B:1:64:2:0:false:0:gui.xaero_default:false:0:0:false\n";
        write(
            &from,
            "xaero/minimap/Multiplayer_alt.example/dim%0/mw$default_1.txt",
            &format!("#\n{basis}waypoint:Tod:D:5:~:5:0:false:1:gui.xaero_default:false:0:0:false\n"),
        );
        write(&from, "xaero/minimap/Multiplayer_alt.example/config.txt", "cfg");
        write(&from, "xaero/minimap/Multiplayer_alt.example/mylauncher-sync.json", "{}");
        write(&from, "xaero/world-map/Multiplayer_alt.example/null/mw$default/0_0.zip", "alt");
        write(&from, "xaero/world-map/Multiplayer_alt.example/null/mw$default/1_0.zip", "neu");
        let target = "xaero/minimap/Multiplayer_neu.example/dim%0/mw$default_1.txt";
        write(
            &to,
            target,
            &format!("#\n{basis}waypoint:Mine:M:9:~:9:3:false:0:gui.xaero_default:false:0:0:false\n"),
        );
        write(&to, "xaero/world-map/Multiplayer_neu.example/null/mw$default/0_0.zip", "ziel");

        let targets = suggest_targets(&xaero::world_folders(&from), &xaero::world_folders(&to));
        assert_eq!(
            targets.get("Multiplayer_alt.example"),
            Some(&Some("Multiplayer_neu.example".to_string()))
        );
        let parts = Parts {
            options: false,
            resource_packs: false,
            shader_packs: false,
            ..Parts::default()
        };
        let sum = copy_game_dir(&from, &to, parts, &targets).unwrap();

        let merged = WaypointFile::load(&to.join(target)).unwrap();
        let names: Vec<_> = merged.waypoints().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Basis", "Mine"], "Doppelte und Todespunkte bleiben draußen");
        assert_eq!((sum.waypoints, sum.map_files), (0, 1));
        let map = to.join("xaero/world-map/Multiplayer_neu.example/null/mw$default");
        assert_eq!(std::fs::read_to_string(map.join("0_0.zip")).unwrap(), "ziel");
        assert_eq!(std::fs::read_to_string(map.join("1_0.zip")).unwrap(), "neu");
        assert!(!to.join("xaero/minimap/Multiplayer_alt.example").exists());
        assert!(!to.join("xaero/minimap/Multiplayer_neu.example/mylauncher-sync.json").exists());

        // Ohne Zuordnung landet die Welt unter ihrem Namen, samt config.txt
        let sum = copy_game_dir(&from, &to, parts, &WorldTargets::new()).unwrap();
        assert_eq!(sum.waypoints, 1);
        let world = to.join("xaero/minimap/Multiplayer_alt.example");
        assert_eq!(std::fs::read_to_string(world.join("config.txt")).unwrap(), "cfg");
    }
}
