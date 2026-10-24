//! Spieleinstellungen aus einer anderen Prism-Instanz übernehmen: Optionen und Tastenbelegung,
//! Ressourcen- und Shaderpakete, Serverliste. Damit muss niemand im neuen Modpack alles
//! wieder von Hand einstellen.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::prism::{self, Prism};

/// Was übernommen werden soll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parts {
    /// `options.txt` (Grafik, Ton, Tastenbelegung, aktive Ressourcenpakete) und `optionsof.txt`
    pub options: bool,
    pub resource_packs: bool,
    /// `shaderpacks/` samt gewähltem Shader (Iris/OptiFine)
    pub shader_packs: bool,
    pub servers: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Self {
            options: true,
            resource_packs: true,
            shader_packs: true,
            servers: false,
        }
    }
}

impl Parts {
    pub fn any(&self) -> bool {
        self.options || self.resource_packs || self.shader_packs || self.servers
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
}

/// Dateien im Format `schlüssel:wert`, die zusammengeführt statt ersetzt werden.
const OPTION_FILES: &[&str] = &["options.txt", "optionsof.txt"];
/// Auswahl des Shaders: Iris bzw. OptiFine.
const SHADER_FILES: &[&str] = &["config/iris.properties", "optionsshaders.txt"];

/// Übernimmt die gewählten Teile von Instanz `from` nach `to`.
pub fn copy_between(prism: &Prism, from: &str, to: &str, parts: Parts) -> Result<Summary> {
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
    )
}

pub fn copy_game_dir(from: &Path, to: &Path, parts: Parts) -> Result<Summary> {
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
    Ok(sum)
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

        let sum = copy_game_dir(&from, &to, Parts::default()).unwrap();

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
}
