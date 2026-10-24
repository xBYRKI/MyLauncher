//! Ablage für Modpacks, Launcher-Konfiguration und Launcher-Updates.
//!
//! ```text
//! <data>/launcher/
//!   site.json                  News, Server, Branding
//!   packs/<id>.json            Pack: Metadaten, Arbeitsstand (Entwurf), Versionen
//!   packs/<id>/<version>.json  veröffentlichtes Manifest – wird nie mehr verändert
//!   blobs/<ab>/<sha1>          Dateiinhalte, nach SHA1 abgelegt (gleiche Mod = eine Datei)
//!   releases.json              Launcher-Versionen
//!   releases/<version>/<exe>   Installer
//! ```
//!
//! Spieler sehen nur veröffentlichte Versionen. Der Entwurf kann beliebig bearbeitet werden,
//! erst „Veröffentlichen“ friert ihn als neue Version ein.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use protocol::pack::{LoaderSpec, Manifest, Memory, PackFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ordner unterhalb von `--data`. Als Teamname daher gesperrt.
pub const DIR: &str = "launcher";

// ---------- Daten ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Brand {
    pub name: String,
    pub tagline: String,
    pub accent: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewsItem {
    pub title: String,
    pub date: String,
    pub body: String,
    pub image: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerEntry {
    pub name: String,
    pub address: String,
    pub instance: String,
}

/// Was der Launcher außer den Packs vom Server bekommt.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Site {
    pub brand: Brand,
    pub news: Vec<NewsItem>,
    pub servers: Vec<ServerEntry>,
    /// Leer = dieser Server selbst, sofern Wegpunkt-Teams eingerichtet sind.
    pub waypoint_server: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftFile {
    pub sha1: String,
    pub size: u64,
    /// `false` = Vorgabedatei, wird beim Spieler nur angelegt, wenn sie fehlt.
    #[serde(default = "yes")]
    pub overwrite: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackVersion {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub published_at: u64,
    pub file_count: usize,
    pub total_size: u64,
}

/// Einstellbare Felder eines Packs (alles außer Dateien und Versionen).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PackMeta {
    pub name: String,
    pub description: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub server: String,
    /// Arbeitsspeicher in MB, 0 = Prism-Vorgabe.
    pub memory: u32,
    /// Ordnername der Prism-Instanz, leer = Packname.
    pub instance_id: String,
    /// Im Launcher anzeigen.
    pub listed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: String,
    #[serde(flatten)]
    pub meta: PackMeta,
    /// Entwurf: Pfad im Spielordner → Datei
    #[serde(default)]
    pub files: BTreeMap<String, DraftFile>,
    #[serde(default)]
    pub versions: Vec<PackVersion>,
    /// Version, die der Launcher bekommt. `None` = noch nie veröffentlicht.
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub pub_date: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
    pub uploaded_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Releases {
    pub list: Vec<Release>,
    /// Version, die `update.json` anbietet. `None` = kein Update anbieten.
    pub current: Option<String>,
}

pub struct Store {
    pub root: PathBuf,
    pub site: Site,
    pub packs: BTreeMap<String, Pack>,
    pub releases: Releases,
}

// ---------- Prüfungen ----------

pub fn valid_pack_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Versionen landen in Dateinamen – daher nur harmlose Zeichen.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && !v.starts_with('.')
        && v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

pub fn valid_sha1(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Pfad im Spielordner: relativ, ohne `..`, ohne Zeichen, die Windows nicht mag.
pub fn clean_file_path(raw: &str) -> Result<String> {
    let path = raw.trim().replace('\\', "/");
    let path = path.trim_matches('/');
    if path.is_empty() || path.len() > 240 {
        bail!("Ungültiger Pfad: {raw}");
    }
    let ok_parts = Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)));
    let bad_char = path
        .chars()
        .any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control());
    let bad_part = path
        .split('/')
        .any(|p| p.is_empty() || p.ends_with(['.', ' ']) || p.ends_with(".part"));
    if !ok_parts || bad_char || bad_part {
        bail!("Ungültiger Pfad: {raw}");
    }
    Ok(path.to_string())
}

/// Dateiname des Installers – landet im Dateisystem und in der URL.
pub fn clean_installer_name(raw: &str, version: &str) -> String {
    let name: String = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if name.len() > 4 && !name.starts_with('.') && name.to_ascii_lowercase().ends_with(".exe") {
        name
    } else {
        format!("MyLauncher_{version}_x64-setup.exe")
    }
}

/// Einzelnes URL-Segment kodieren (Leerzeichen, Umlaute, `#` …).
pub fn url_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------- Dateien ----------

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("{} ist kaputt", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("{} nicht lesbar", path.display())),
    }
}

/// Atomar schreiben: erst `.tmp`, dann umbenennen.
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Store {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join(DIR);
        for sub in ["packs", "blobs/tmp", "releases"] {
            std::fs::create_dir_all(root.join(sub))?;
        }
        // Abgebrochene Uploads wegräumen
        if let Ok(entries) = std::fs::read_dir(root.join("blobs/tmp")) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let mut packs = BTreeMap::new();
        for entry in std::fs::read_dir(root.join("packs"))?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(pack) = read::<Pack>(&path)? {
                    packs.insert(pack.id.clone(), pack);
                }
            }
        }
        Ok(Self {
            site: read(&root.join("site.json"))?.unwrap_or_default(),
            releases: read(&root.join("releases.json"))?.unwrap_or_default(),
            packs,
            root,
        })
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("blobs/tmp")
    }

    pub fn blob_path(&self, sha1: &str) -> PathBuf {
        self.root.join("blobs").join(&sha1[..2]).join(sha1)
    }

    pub fn manifest_path(&self, id: &str, version: &str) -> PathBuf {
        self.root.join("packs").join(id).join(format!("{version}.json"))
    }

    pub fn release_path(&self, r: &Release) -> PathBuf {
        self.root.join("releases").join(&r.version).join(&r.file)
    }

    pub fn save_site(&self) -> Result<()> {
        write_json(&self.root.join("site.json"), &self.site)
    }

    pub fn save_releases(&self) -> Result<()> {
        write_json(&self.root.join("releases.json"), &self.releases)
    }

    pub fn save_pack(&mut self, id: &str) -> Result<()> {
        let pack = self.packs.get_mut(id).context("Pack nicht gefunden")?;
        pack.updated_at = crate::now_millis();
        write_json(&self.root.join("packs").join(format!("{id}.json")), pack)
    }

    pub fn delete_pack(&mut self, id: &str) -> Result<()> {
        if self.packs.remove(id).is_none() {
            bail!("Pack nicht gefunden");
        }
        std::fs::remove_file(self.root.join("packs").join(format!("{id}.json")))?;
        let _ = std::fs::remove_dir_all(self.root.join("packs").join(id));
        Ok(())
    }

    /// Fertige Datei aus `blobs/tmp` unter ihrer SHA1 ablegen.
    pub fn adopt_blob(&self, tmp: &Path, sha1: &str) -> Result<()> {
        let dest = self.blob_path(sha1);
        if dest.exists() {
            std::fs::remove_file(tmp)?;
        } else {
            std::fs::create_dir_all(dest.parent().expect("Blob-Ordner"))?;
            std::fs::rename(tmp, &dest)?;
        }
        Ok(())
    }

    pub fn load_manifest(&self, id: &str, version: &str) -> Result<Option<Manifest>> {
        read(&self.manifest_path(id, version))
    }

    /// Manifest aus dem Entwurf bauen. Datei-URLs sind relativ zum Manifest
    /// (`/launcher/packs/<id>/manifest.json` → `/launcher/packs/<id>/files/<sha1>/<name>`).
    pub fn draft_manifest(pack: &Pack, version: &str) -> Manifest {
        let files = pack
            .files
            .iter()
            .map(|(path, f)| {
                let name = path.rsplit('/').next().unwrap_or(path);
                PackFile {
                    path: path.clone(),
                    url: format!("files/{}/{}", f.sha1, url_segment(name)),
                    sha1: Some(f.sha1.clone()),
                    size: Some(f.size),
                    overwrite: (!f.overwrite).then_some(false),
                }
            })
            .collect();
        let loader = if pack.meta.loader.kind.is_empty() || pack.meta.loader.kind == "vanilla" {
            LoaderSpec {
                kind: "vanilla".into(),
                version: String::new(),
            }
        } else {
            pack.meta.loader.clone()
        };
        Manifest {
            format_version: 1,
            id: pack.id.clone(),
            name: pack.meta.name.clone(),
            version: version.to_string(),
            description: pack.meta.description.clone(),
            instance_id: pack.meta.instance_id.clone(),
            minecraft: pack.meta.minecraft.clone(),
            loader,
            server: pack.meta.server.clone(),
            memory: (pack.meta.memory > 0).then_some(Memory {
                min: Some(512),
                max: pack.meta.memory,
            }),
            files,
        }
    }

    pub fn publish(&mut self, id: &str, version: &str, notes: &str) -> Result<()> {
        let pack = self.packs.get(id).context("Pack nicht gefunden")?;
        if !valid_version(version) {
            bail!("Ungültige Version – erlaubt sind Buchstaben, Ziffern und . - _ +");
        }
        if pack.versions.iter().any(|v| v.version == version) {
            bail!("Version {version} gibt es schon");
        }
        check_meta(&pack.meta)?;
        let manifest = Self::draft_manifest(pack, version);
        let entry = PackVersion {
            version: version.to_string(),
            notes: notes.trim().to_string(),
            published_at: crate::now_millis(),
            file_count: manifest.files.len(),
            total_size: pack.files.values().map(|f| f.size).sum(),
        };
        write_json(&self.manifest_path(id, version), &manifest)?;
        let pack = self.packs.get_mut(id).expect("geprüft");
        pack.versions.push(entry);
        pack.current = Some(version.to_string());
        self.save_pack(id)
    }

    /// Entwurf auf den Stand einer veröffentlichten Version zurücksetzen.
    pub fn reset_draft(&mut self, id: &str, version: &str) -> Result<()> {
        let manifest = self
            .load_manifest(id, version)?
            .context("Version nicht gefunden")?;
        let pack = self.packs.get_mut(id).context("Pack nicht gefunden")?;
        pack.files = draft_files(&manifest);
        pack.meta.name = manifest.name;
        pack.meta.description = manifest.description;
        pack.meta.minecraft = manifest.minecraft;
        pack.meta.loader = manifest.loader;
        pack.meta.server = manifest.server;
        pack.meta.memory = manifest.memory.map(|m| m.max).unwrap_or(0);
        pack.meta.instance_id = manifest.instance_id;
        self.save_pack(id)
    }

    /// Alle SHA1s, die noch gebraucht werden (Entwürfe und alle Versionen).
    pub fn referenced_blobs(&self) -> Result<HashSet<String>> {
        let mut used = HashSet::new();
        for pack in self.packs.values() {
            used.extend(pack.files.values().map(|f| f.sha1.clone()));
            for v in &pack.versions {
                if let Some(m) = self.load_manifest(&pack.id, &v.version)? {
                    used.extend(m.files.into_iter().filter_map(|f| f.sha1));
                }
            }
        }
        Ok(used)
    }

    /// Nicht mehr benötigte Dateien löschen. Gibt Anzahl und Bytes zurück.
    pub fn collect_garbage(&self) -> Result<(usize, u64)> {
        let used = self.referenced_blobs()?;
        let (mut count, mut bytes) = (0, 0);
        for dir in std::fs::read_dir(self.root.join("blobs"))?.flatten() {
            if dir.file_name() == "tmp" || !dir.path().is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(dir.path())?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !used.contains(&name) {
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    std::fs::remove_file(entry.path())?;
                    count += 1;
                }
            }
        }
        Ok((count, bytes))
    }

    pub fn blob_usage(&self) -> (usize, u64) {
        let (mut count, mut bytes) = (0, 0);
        let Ok(dirs) = std::fs::read_dir(self.root.join("blobs")) else {
            return (0, 0);
        };
        for dir in dirs.flatten() {
            if dir.file_name() == "tmp" {
                continue;
            }
            for entry in std::fs::read_dir(dir.path()).into_iter().flatten().flatten() {
                count += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        (count, bytes)
    }

    /// Antwort für `/launcher/config.json`.
    pub fn launcher_config(&self, waypoint_default: &str) -> Value {
        let mut out = serde_json::Map::new();
        let brand = &self.site.brand;
        let mut b = serde_json::Map::new();
        for (key, value) in [
            ("name", &brand.name),
            ("tagline", &brand.tagline),
            ("accent", &brand.accent),
        ] {
            if !value.trim().is_empty() {
                b.insert(key.into(), value.trim().into());
            }
        }
        if !b.is_empty() {
            out.insert("brand".into(), b.into());
        }
        let packs: Vec<Value> = self
            .packs
            .values()
            .filter(|p| p.meta.listed && p.current.is_some())
            .map(|p| serde_json::json!({ "manifest": format!("packs/{}/manifest.json", p.id) }))
            .collect();
        out.insert("packs".into(), packs.into());
        let news: Vec<Value> = self
            .site
            .news
            .iter()
            .map(|n| {
                let mut item = serde_json::Map::new();
                for (key, value) in [
                    ("title", &n.title),
                    ("date", &n.date),
                    ("body", &n.body),
                    ("image", &n.image),
                    ("url", &n.url),
                ] {
                    if !value.trim().is_empty() {
                        item.insert(key.into(), value.trim().into());
                    }
                }
                Value::Object(item)
            })
            .collect();
        out.insert("news".into(), news.into());
        let servers: Vec<Value> = self
            .site
            .servers
            .iter()
            .map(|s| {
                let mut item = serde_json::json!({ "name": s.name, "address": s.address });
                if !s.instance.trim().is_empty() {
                    item["instance"] = s.instance.trim().into();
                }
                item
            })
            .collect();
        out.insert("servers".into(), servers.into());
        let waypoint = if self.site.waypoint_server.trim().is_empty() {
            waypoint_default
        } else {
            self.site.waypoint_server.trim()
        };
        if !waypoint.is_empty() {
            out.insert("waypointServer".into(), waypoint.into());
        }
        Value::Object(out)
    }
}

pub fn draft_files(manifest: &Manifest) -> BTreeMap<String, DraftFile> {
    manifest
        .files
        .iter()
        .filter_map(|f| {
            Some((
                f.path.clone(),
                DraftFile {
                    sha1: f.sha1.clone()?,
                    size: f.size.unwrap_or(0),
                    overwrite: f.overwrite != Some(false),
                },
            ))
        })
        .collect()
}

pub fn check_meta(meta: &PackMeta) -> Result<()> {
    if meta.name.trim().is_empty() {
        bail!("Name fehlt");
    }
    if meta.minecraft.trim().is_empty() {
        bail!("Minecraft-Version fehlt");
    }
    let kind = meta.loader.kind.as_str();
    if !kind.is_empty() && kind != "vanilla" {
        if protocol::pack::loader_uid(kind).is_none() {
            bail!("Unbekannter Loader \"{kind}\"");
        }
        if meta.loader.version.trim().is_empty() {
            bail!("Loader-Version fehlt");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pfade_werden_geprueft() {
        assert_eq!(clean_file_path("mods/a.jar").unwrap(), "mods/a.jar");
        assert_eq!(clean_file_path("\\config\\x.toml").unwrap(), "config/x.toml");
        for bad in ["../x", "mods/../../x", "C:/x", "a//b", "mods/x.jar.part", "a/./b", "x?", ""] {
            assert!(clean_file_path(bad).is_err(), "{bad} müsste abgelehnt werden");
        }
    }

    #[test]
    fn ids_und_versionen() {
        assert!(valid_pack_id("survival-1"));
        assert!(!valid_pack_id("Survival"));
        assert!(!valid_pack_id("-x"));
        assert!(valid_version("1.2.0-beta.1"));
        assert!(!valid_version("../1"));
        assert!(!valid_version("1/2"));
    }

    #[test]
    fn installer_name_bleibt_harmlos() {
        assert_eq!(clean_installer_name("C:\\x\\Setup 1.exe", "1"), "Setup1.exe");
        assert_eq!(clean_installer_name("evil.bat", "0.3.0"), "MyLauncher_0.3.0_x64-setup.exe");
        assert_eq!(clean_installer_name(".exe", "1"), "MyLauncher_1_x64-setup.exe");
    }

    #[test]
    fn url_segment_kodiert() {
        assert_eq!(url_segment("Mein Mod #1.jar"), "Mein%20Mod%20%231.jar");
        assert_eq!(url_segment("ä"), "%C3%A4");
    }
}
