//! Xaero's Minimap: Wegpunktdateien lesen und schreiben.
//!
//! Aufbau im Spielordner:
//! ```text
//! xaero/minimap/<Welt>/dim%0/mw$default_1.txt     (neuere Versionen)
//! XaeroWaypoints/<Welt>/dim%0/mw$default_1.txt     (ältere Versionen)
//! ```
//! `<Welt>` ist z. B. `Multiplayer_play.example.net` oder der Name einer Einzelspielerwelt.
//! Zeilenformat:
//! `waypoint:name:initials:x:y:z:color:disabled:type:set:rotate_on_tp:tp_yaw:visibility_type:destination`

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Xaero ersetzt `:` in Namen durch `§§`.
fn decode_name(s: &str) -> String {
    s.replace("§§", ":")
}

fn encode_name(s: &str) -> String {
    s.replace(':', "§§")
}

pub const DEATH_TYPE: u8 = 1;
pub const DEFAULT_SET: &str = "gui.xaero_default";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waypoint {
    pub name: String,
    pub initials: String,
    pub x: i64,
    pub y: String,
    pub z: i64,
    pub color: u8,
    pub disabled: bool,
    pub kind: u8,
    pub set: String,
    pub extra: Vec<String>,
}

impl Waypoint {
    fn parse(line: &str) -> Option<Self> {
        let fields: Vec<&str> = line.strip_prefix("waypoint:")?.split(':').collect();
        if fields.len() < 9 {
            return None;
        }
        Some(Self {
            name: decode_name(fields[0]),
            initials: decode_name(fields[1]),
            x: fields[2].parse().ok()?,
            y: fields[3].to_string(),
            z: fields[4].parse().ok()?,
            color: fields[5].parse().ok()?,
            disabled: fields[6] == "true",
            kind: fields[7].parse().ok()?,
            set: fields[8].to_string(),
            extra: fields[9..].iter().map(|s| s.to_string()).collect(),
        })
    }

    fn to_line(&self) -> String {
        let mut parts = vec![
            "waypoint".to_string(),
            encode_name(&self.name),
            encode_name(&self.initials),
            self.x.to_string(),
            self.y.clone(),
            self.z.to_string(),
            self.color.to_string(),
            self.disabled.to_string(),
            self.kind.to_string(),
            self.set.clone(),
        ];
        parts.extend(self.extra.iter().cloned());
        parts.join(":")
    }
}

#[derive(Clone, Debug)]
enum Line {
    Waypoint(Waypoint),
    Other(String),
}

/// Eine Wegpunktdatei; unbekannte Zeilen (Kommentare, `sets:` …) bleiben erhalten.
#[derive(Clone, Debug)]
pub struct WaypointFile {
    lines: Vec<Line>,
}

const HEADER: &str = "#\n#waypoint:name:initials:x:y:z:color:disabled:type:set:rotate_on_tp:tp_yaw:visibility_type:destination\n#";

impl WaypointFile {
    pub fn parse(text: &str) -> Self {
        let lines = text
            .lines()
            .map(|l| match Waypoint::parse(l.trim_end()) {
                Some(w) => Line::Waypoint(w),
                None => Line::Other(l.to_string()),
            })
            .collect();
        Self { lines }
    }

    pub fn new_empty() -> Self {
        Self::parse(HEADER)
    }

    pub fn load(path: &Path) -> Option<Self> {
        std::fs::read_to_string(path).ok().map(|t| Self::parse(&t))
    }

    pub fn waypoints(&self) -> impl Iterator<Item = &Waypoint> {
        self.lines.iter().filter_map(|l| match l {
            Line::Waypoint(w) => Some(w),
            Line::Other(_) => None,
        })
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&Waypoint) -> bool) {
        self.lines.retain(|l| match l {
            Line::Waypoint(w) => keep(w),
            Line::Other(_) => true,
        });
    }

    pub fn push(&mut self, w: Waypoint) {
        // Hinter den letzten Wegpunkt bzw. ans Ende (vor abschließende Leerzeilen).
        let pos = self
            .lines
            .iter()
            .rposition(|l| {
                matches!(l, Line::Waypoint(_))
                    || matches!(l, Line::Other(s) if !s.trim().is_empty())
            })
            .map(|p| p + 1)
            .unwrap_or(self.lines.len());
        self.lines.insert(pos, Line::Waypoint(w));
    }

    /// Falls die Datei eine `sets:`-Zeile hat, das Set dort eintragen.
    pub fn ensure_set(&mut self, set: &str) {
        for line in &mut self.lines {
            if let Line::Other(s) = line {
                if let Some(rest) = s.strip_prefix("sets:") {
                    if !rest.split(':').any(|x| x == set) {
                        s.push(':');
                        s.push_str(set);
                    }
                }
            }
        }
    }

    pub fn render(&self) -> String {
        let mut out: Vec<String> = self
            .lines
            .iter()
            .map(|l| match l {
                Line::Waypoint(w) => w.to_line(),
                Line::Other(s) => s.clone(),
            })
            .collect();
        out.push(String::new());
        out.join("\n")
    }
}

// ---------- Ordner ----------

/// Wurzel der Xaero-Wegpunkte im Spielordner (neu: `xaero/minimap`, alt: `XaeroWaypoints`).
pub fn minimap_root(game_dir: &Path) -> PathBuf {
    let modern = game_dir.join("xaero").join("minimap");
    let legacy = game_dir.join("XaeroWaypoints");
    if !modern.exists() && legacy.exists() {
        legacy
    } else {
        modern
    }
}

/// Alle Wegpunktdateien einer Welt: (Dimensionsordner, Datei-Stem, Pfad)
pub fn world_files(world_dir: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = vec![];
    let Ok(dims) = std::fs::read_dir(world_dir) else {
        return out;
    };
    for dim in dims.flatten() {
        let dim_name = dim.file_name().to_string_lossy().into_owned();
        if !dim_name.starts_with("dim%") || !dim.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dim.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "txt") {
                let stem = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                out.push((dim_name.clone(), stem, path));
            }
        }
    }
    out.sort();
    out
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldInfo {
    /// Ordnername, dient auch als Raumname auf dem Sync-Server.
    pub folder: String,
    /// Anzeigename, z. B. `play.example.net`.
    pub display: String,
    pub multiplayer: bool,
    pub waypoints: usize,
    pub shared: usize,
}

pub fn display_name(folder: &str) -> String {
    folder
        .strip_prefix("Multiplayer_")
        .unwrap_or(folder)
        .to_string()
}

pub fn list_worlds(game_dir: &Path, is_shared: impl Fn(&Waypoint) -> bool) -> Vec<WorldInfo> {
    let root = minimap_root(game_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return vec![];
    };
    let mut worlds: Vec<WorldInfo> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().into_owned();
            if folder == "backup" {
                return None;
            }
            let files = world_files(&e.path());
            if files.is_empty() {
                return None;
            }
            let all: Vec<Waypoint> = files
                .iter()
                .filter_map(|(_, _, p)| WaypointFile::load(p))
                .flat_map(|f| f.waypoints().cloned().collect::<Vec<_>>())
                .collect();
            Some(WorldInfo {
                display: display_name(&folder),
                multiplayer: folder.starts_with("Multiplayer_"),
                waypoints: all.len(),
                shared: all.iter().filter(|w| is_shared(w)).count(),
                folder,
            })
        })
        .collect();
    worlds.sort_by(|a, b| {
        b.multiplayer
            .cmp(&a.multiplayer)
            .then_with(|| a.display.cmp(&b.display))
    });
    worlds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_render_roundtrip() {
        let text = "#\n#waypoint:name:...\n#\nwaypoint:Wüste §§ Portal:W:-1925:78:1801:2:false:0:gui.xaero_default:false:0:0:false\nwaypoint:Base:B:-181:~:430:15:true:0:Team:false:0:0:false\n";
        let f = WaypointFile::parse(text);
        let wps: Vec<_> = f.waypoints().collect();
        assert_eq!(wps.len(), 2);
        assert_eq!(wps[0].name, "Wüste : Portal");
        assert_eq!(wps[1].y, "~");
        assert!(wps[1].disabled);
        assert_eq!(f.render(), text);
    }
}
