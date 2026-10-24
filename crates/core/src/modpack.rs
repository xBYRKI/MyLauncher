//! Eigener Modpack-Sync: Ein `manifest.json` (z. B. auf GitHub Pages oder deinem Webspace)
//! beschreibt Minecraft-Version, Loader und Dateien. Daraus wird eine Prism-Instanz
//! angelegt bzw. aktualisiert. Nur Dateien aus dem Manifest werden verwaltet – eigene
//! Mods/Configs des Spielers bleiben unangetastet.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::ini::Ini;
use crate::prism::{self, Prism};

pub const STATE_FILE: &str = "mylauncher-pack.json";
const PARALLEL_DOWNLOADS: usize = 4;

// Manifest-Typen teilen sich Launcher, Server und packtool.
pub use protocol::pack::{loader_uid, LoaderSpec, Manifest, Memory, PackFile};

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP-Client")
}

pub fn to_url(source: &str) -> Result<Url> {
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("file:")
    {
        return Ok(Url::parse(source)?);
    }
    let abs = std::path::absolute(source)?;
    Url::from_file_path(&abs).map_err(|_| anyhow!("Ungültiger Pfad: {source}"))
}

async fn fetch_text(url: &Url) -> Result<String> {
    if url.scheme() == "file" {
        let path = url.to_file_path().map_err(|_| anyhow!("Ungültiger Pfad"))?;
        return tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("{} nicht lesbar", path.display()));
    }
    let res = http()
        .get(url.clone())
        .header("Cache-Control", "no-cache")
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    if !res.status().is_success() {
        bail!("HTTP {} für {url}", res.status());
    }
    Ok(res.text().await?)
}

/// Text von einer URL oder einem lokalen Pfad laden (Manifeste, News).
pub async fn fetch_source(source: &str) -> Result<String> {
    fetch_text(&to_url(source)?).await
}

fn sanitize_folder(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| {
            !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if trimmed.is_empty() {
        "pack".into()
    } else {
        trimmed
    }
}

fn validate(mut m: Manifest) -> Result<Manifest> {
    for (field, value) in [
        ("id", &m.id),
        ("name", &m.name),
        ("version", &m.version),
        ("minecraft", &m.minecraft),
    ] {
        if value.trim().is_empty() {
            bail!("Manifest: Feld \"{field}\" fehlt");
        }
    }
    m.loader.kind = m.loader.kind.to_lowercase();
    if !m.loader.kind.is_empty() && m.loader.kind != "vanilla" {
        if loader_uid(&m.loader.kind).is_none() {
            bail!("Manifest: unbekannter Loader \"{}\"", m.loader.kind);
        }
        if m.loader.version.is_empty() {
            bail!("Manifest: loader.version fehlt");
        }
    }
    for f in &m.files {
        if f.path.is_empty() || f.url.is_empty() {
            bail!("Manifest: jede Datei braucht \"path\" und \"url\"");
        }
        relative_path(&f.path)?;
    }
    let base = if m.instance_id.is_empty() {
        m.name.clone()
    } else {
        m.instance_id.clone()
    };
    m.instance_id = sanitize_folder(&base);
    Ok(m)
}

pub async fn load_manifest(source: &str) -> Result<(Manifest, Url)> {
    let url = to_url(source)?;
    let text = fetch_text(&url).await?;
    let manifest: Manifest =
        serde_json::from_str(&text).context("Manifest ist kein gültiges JSON")?;
    Ok((validate(manifest)?, url))
}

// ---------- Dateien ----------

/// Nur einfache relative Pfade erlauben (kein `..`, keine Laufwerke).
fn relative_path(rel: &str) -> Result<PathBuf> {
    let path = Path::new(rel);
    if path.components().all(|c| matches!(c, Component::Normal(_))) {
        Ok(path.to_path_buf())
    } else {
        Err(anyhow!("Ungültiger Pfad im Manifest: {rel}"))
    }
}

pub async fn sha1_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn explain_fs_error(err: std::io::Error, file: &Path) -> anyhow::Error {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if matches!(err.kind(), std::io::ErrorKind::PermissionDenied) || err.raw_os_error() == Some(32)
    {
        anyhow!("{name} ist gesperrt – läuft Minecraft noch? Bitte schließen und erneut versuchen.")
    } else {
        anyhow!("{name}: {err}")
    }
}

async fn download(
    client: &reqwest::Client,
    url: &Url,
    dest: &Path,
    expected: Option<&str>,
    bytes: &AtomicU64,
) -> Result<String> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    let mut hasher = Sha1::new();
    let result: Result<()> = async {
        let mut out = tokio::fs::File::create(&tmp).await?;
        if url.scheme() == "file" {
            let data = tokio::fs::read(url.to_file_path().map_err(|_| anyhow!("Ungültiger Pfad"))?)
                .await?;
            hasher.update(&data);
            bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            out.write_all(&data).await?;
        } else {
            let res = client
                .get(url.clone())
                .timeout(Duration::from_secs(300))
                .send()
                .await?;
            if !res.status().is_success() {
                bail!("Download fehlgeschlagen (HTTP {}): {url}", res.status());
            }
            let mut body = res.bytes_stream();
            while let Some(chunk) = body.try_next().await? {
                hasher.update(&chunk);
                bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                out.write_all(&chunk).await?;
            }
        }
        out.flush().await?;
        Ok(())
    }
    .await;

    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    let actual = hex::encode(hasher.finalize());
    if let Some(exp) = expected {
        if !actual.eq_ignore_ascii_case(exp) {
            let _ = tokio::fs::remove_file(&tmp).await;
            bail!("Prüfsumme stimmt nicht: {}", dest.display());
        }
    }
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| explain_fs_error(e, dest))?;
    Ok(actual)
}

// ---------- Prism-Instanz ----------

fn pack_components(m: &Manifest) -> Value {
    let mut components =
        vec![json!({ "uid": "net.minecraft", "version": m.minecraft, "important": true })];
    if let Some(uid) = loader_uid(&m.loader.kind) {
        components.push(json!({ "uid": uid, "version": m.loader.version }));
    }
    json!({ "components": components, "formatVersion": 1 })
}

fn ensure_instance(prism: &Prism, m: &Manifest, brand: &str) -> Result<PathBuf> {
    let dir = prism.instances_dir.join(&m.instance_id);
    let cfg_path = dir.join("instance.cfg");
    let pack_path = dir.join("mmc-pack.json");
    let created = !cfg_path.exists();

    if created {
        std::fs::create_dir_all(dir.join("minecraft"))?;
        let mut ini = Ini::default();
        for (k, v) in [
            ("ConfigVersion", "1.3"),
            ("InstanceType", "OneSix"),
            ("name", m.name.as_str()),
            ("iconKey", "default"),
        ] {
            ini.set("General", k, v);
        }
        ini.set(
            "General",
            "notes",
            &format!(
                "Verwaltet von {brand}. Dateien aus dem Modpack werden bei Updates überschrieben."
            ),
        );
        if let Some(mem) = &m.memory {
            ini.set("General", "OverrideMemory", "true");
            ini.set(
                "General",
                "MinMemAlloc",
                &mem.min.unwrap_or(512).to_string(),
            );
            ini.set("General", "MaxMemAlloc", &mem.max.to_string());
        }
        ini.save(&cfg_path)?;
    }

    // Version/Loader nur neu schreiben, wenn sie sich geändert haben – Prism löst
    // Abhängigkeiten (LWJGL, Intermediary, …) beim nächsten Start selbst auf.
    let wanted = pack_components(m);
    let (cur_mc, cur_loader) = prism::describe_pack(prism::read_json(&pack_path).as_ref());
    let (want_mc, want_loader) = prism::describe_pack(Some(&wanted));
    let same_loader = cur_loader.as_ref().map(|l| (&l.name, &l.version))
        == want_loader.as_ref().map(|l| (&l.name, &l.version));
    if created || cur_mc != want_mc || !same_loader {
        std::fs::write(&pack_path, serde_json::to_string_pretty(&wanted)?)?;
    }
    Ok(dir)
}

// ---------- Öffentliche API ----------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackStatus {
    pub source: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub minecraft: String,
    pub loader: String,
    pub server: String,
    pub instance_id: String,
    pub file_count: usize,
    pub installed_version: Option<String>,
    pub up_to_date: bool,
    pub error: Option<String>,
}

/// Status eines Packs für die UI (ohne etwas zu verändern).
pub async fn pack_status(prism: &Prism, source: &str) -> PackStatus {
    match load_manifest(source).await {
        Ok((m, _)) => {
            let state =
                prism::read_json(&prism.instances_dir.join(&m.instance_id).join(STATE_FILE));
            let installed = state.and_then(|s| s["version"].as_str().map(String::from));
            PackStatus {
                source: source.into(),
                up_to_date: installed.as_deref() == Some(m.version.as_str()),
                installed_version: installed,
                file_count: m.files.len(),
                loader: if m.loader.kind.is_empty() {
                    "vanilla".into()
                } else {
                    m.loader.kind
                },
                id: m.id,
                name: m.name,
                description: m.description,
                version: m.version,
                minecraft: m.minecraft,
                server: m.server,
                instance_id: m.instance_id,
                error: None,
            }
        }
        Err(e) => PackStatus {
            source: source.into(),
            id: String::new(),
            name: String::new(),
            description: String::new(),
            version: String::new(),
            minecraft: String::new(),
            loader: String::new(),
            server: String::new(),
            instance_id: String::new(),
            file_count: 0,
            installed_version: None,
            up_to_date: false,
            error: Some(format!("{e:#}")),
        },
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
    pub file: String,
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub instance_id: String,
    pub version: String,
    pub server: String,
    pub downloaded: usize,
    pub removed: usize,
}

struct Todo {
    file: PackFile,
    target: PathBuf,
    keep: bool,
}

// ---------- Abgleich: Vorschau, Integrität, fremde Dateien ----------

/// Was beim nächsten Sync mit einer Datei passieren würde.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// Neu im Pack.
    Add,
    /// Im Pack durch eine andere Fassung ersetzt.
    Update,
    /// Lokal beschädigt, gelöscht oder von Hand geändert – wird wiederhergestellt.
    Repair,
    /// Nicht mehr im Pack – wird entfernt.
    Remove,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Datei im Spielordner, die nicht aus dem Manifest kommt – meist selbst installierte Mods.
/// Wird vom Sync nie angefasst, hier aber sichtbar gemacht (hilft bei der Fehlersuche).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extra {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackPlan {
    pub source: String,
    pub name: String,
    pub instance_id: String,
    pub version: String,
    pub installed_version: Option<String>,
    /// Alles, was der Sync anfassen würde.
    pub changes: Vec<Change>,
    /// Dateien, die schon stimmen.
    pub unchanged: usize,
    /// Zu ladende Bytes, soweit das Manifest Größen angibt.
    pub download_bytes: u64,
    pub extras: Vec<Extra>,
    /// Es gibt mehr fremde Dateien, als aufgelistet sind.
    pub extras_truncated: bool,
}

/// Ordner, in denen fremde Dateien interessant sind.
const EXTRA_DIRS: &[&str] = &["mods", "resourcepacks", "shaderpacks"];
const EXTRA_LIMIT: usize = 60;

fn scan_extras(game: &Path, managed: &HashSet<String>) -> (Vec<Extra>, bool) {
    let mut out: Vec<Extra> = vec![];
    let mut truncated = false;
    for dir in EXTRA_DIRS {
        let mut stack = vec![game.join(dir)];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                // .index/.cache gehören Prism bzw. Loadern, .part sind halbe Downloads.
                if name.starts_with('.') || name.ends_with(".part") {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Ok(rel) = path.strip_prefix(game) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if managed.contains(&rel) {
                    continue;
                }
                if out.len() >= EXTRA_LIMIT {
                    truncated = true;
                    continue;
                }
                out.push(Extra {
                    path: rel,
                    size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, truncated)
}

/// Ergebnis der Prüfphase – von Vorschau und Sync gemeinsam genutzt, damit die
/// Vorschau nie etwas anderes anzeigt, als der Sync dann tut.
struct Work {
    todo: Vec<Todo>,
    /// Dateien, die schon stimmen (Pfad -> SHA1); wird beim Sync um die Downloads ergänzt.
    managed: BTreeMap<String, String>,
    changes: Vec<Change>,
    unchanged: usize,
    /// Verwaltete Dateien, die nicht mehr im Manifest stehen.
    removals: Vec<String>,
}

type OnCheck<'a> = &'a (dyn Fn(usize, usize, &str) + Send + Sync);

async fn examine(
    game: &Path,
    manifest: &Manifest,
    old_files: &BTreeMap<String, String>,
    on_check: OnCheck<'_>,
) -> Result<Work> {
    let mut work = Work {
        todo: vec![],
        managed: BTreeMap::new(),
        changes: vec![],
        unchanged: 0,
        removals: vec![],
    };
    let total = manifest.files.len();
    for (i, f) in manifest.files.iter().enumerate() {
        on_check(i, total, &f.path);
        let target = game.join(relative_path(&f.path)?);
        let keep = f.overwrite == Some(false);
        let exists = target.exists();
        if exists {
            // Vorgabedateien (options.txt o. ä.) gehören nach dem Anlegen dem Spieler.
            if keep {
                work.unchanged += 1;
                continue;
            }
            if let Some(expected) = &f.sha1 {
                if sha1_file(&target).await?.eq_ignore_ascii_case(expected) {
                    work.managed.insert(f.path.clone(), expected.to_lowercase());
                    work.unchanged += 1;
                    continue;
                }
            }
        }
        // Stand der letzten Installation entscheidet, ob das Pack die Datei geändert hat
        // (Update) oder ob sie hier lokal abweicht (Repair).
        let known = old_files.get(&f.path);
        let kind = match (known, &f.sha1) {
            (None, _) => ChangeKind::Add,
            (Some(old), Some(want)) if old.eq_ignore_ascii_case(want) => ChangeKind::Repair,
            (Some(_), Some(_)) => ChangeKind::Update,
            // Ohne Prüfsumme im Manifest lässt sich das nicht unterscheiden.
            (Some(_), None) => ChangeKind::Update,
        };
        work.changes.push(Change {
            path: f.path.clone(),
            kind,
            size: f.size,
        });
        work.todo.push(Todo {
            file: f.clone(),
            target,
            keep,
        });
    }

    // Was wir verwaltet haben, aber nicht mehr im Manifest steht, wird entfernt.
    let wanted: HashSet<&String> = manifest
        .files
        .iter()
        .filter(|f| f.overwrite != Some(false))
        .map(|f| &f.path)
        .collect();
    for rel in old_files.keys().filter(|r| !wanted.contains(r)) {
        work.removals.push(rel.clone());
        if relative_path(rel).is_ok_and(|p| game.join(p).exists()) {
            work.changes.push(Change {
                path: rel.clone(),
                kind: ChangeKind::Remove,
                size: None,
            });
        }
    }
    work.changes.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.path.cmp(&b.path)));
    Ok(work)
}

/// Abgleich ohne Schreibzugriff: zeigt, was ein Sync tun würde, prüft dabei alle
/// verwalteten Dateien per SHA1 und listet fremde Dateien auf.
pub async fn plan_pack(
    prism: &Prism,
    source: &str,
    on_progress: impl Fn(Progress) + Send + Sync,
) -> Result<PackPlan> {
    let (manifest, _) = load_manifest(source).await?;
    let dir = prism.instances_dir.join(&manifest.instance_id);
    let game = prism::game_dir(&dir);
    let state = prism::read_json(&dir.join(STATE_FILE));
    let installed_version = state
        .as_ref()
        .and_then(|s| s["version"].as_str().map(String::from));
    let old_files: BTreeMap<String, String> = state
        .and_then(|s| serde_json::from_value(s["files"].clone()).ok())
        .unwrap_or_default();

    let work = examine(&game, &manifest, &old_files, &|done, total, file| {
        on_progress(Progress {
            phase: "check",
            done,
            total,
            file: file.into(),
            bytes: 0,
            total_bytes: 0,
        })
    })
    .await?;

    let managed_paths: HashSet<String> = manifest
        .files
        .iter()
        .map(|f| f.path.replace('\\', "/"))
        .collect();
    let (extras, extras_truncated) = if game.is_dir() {
        scan_extras(&game, &managed_paths)
    } else {
        (vec![], false)
    };

    Ok(PackPlan {
        download_bytes: work.todo.iter().filter_map(|t| t.file.size).sum(),
        changes: work.changes,
        unchanged: work.unchanged,
        source: source.into(),
        name: manifest.name,
        instance_id: manifest.instance_id,
        version: manifest.version,
        installed_version,
        extras,
        extras_truncated,
    })
}

/// Installiert/aktualisiert ein Pack als Prism-Instanz.
pub async fn sync_pack(
    prism: &Prism,
    source: &str,
    brand: &str,
    on_progress: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<SyncResult> {
    let (manifest, base_url) = load_manifest(source).await?;
    let dir = ensure_instance(prism, &manifest, brand)?;
    let game = prism::game_dir(&dir);
    let state_path = dir.join(STATE_FILE);
    let old_files: BTreeMap<String, String> = prism::read_json(&state_path)
        .and_then(|s| serde_json::from_value(s["files"].clone()).ok())
        .unwrap_or_default();

    let on_progress = Arc::new(on_progress);
    let progress = {
        let on_progress = on_progress.clone();
        move |phase, done, total, file: &str, bytes, total_bytes| {
            on_progress(Progress {
                phase,
                done,
                total,
                file: file.into(),
                bytes,
                total_bytes,
            })
        }
    };

    // 1) Prüfen, welche Dateien fehlen oder veraltet sind – gleiche Logik wie die Vorschau
    let Work {
        todo,
        mut managed,
        removals,
        ..
    } = examine(&game, &manifest, &old_files, &|done, total, file| {
        progress("check", done, total, file, 0, 0)
    })
    .await?;

    // 2) Herunterladen (parallel)
    let total_bytes: u64 = todo.iter().filter_map(|t| t.file.size).sum();
    let bytes = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let client = http();
    let count = todo.len();
    let jobs = todo.into_iter().map(|t| {
        let (client, bytes, done, base_url, on_progress) = (
            client.clone(),
            bytes.clone(),
            done.clone(),
            base_url.clone(),
            on_progress.clone(),
        );
        async move {
            let url = base_url.join(&t.file.url)?;
            let sha = download(&client, &url, &t.target, t.file.sha1.as_deref(), &bytes).await?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            on_progress(Progress {
                phase: "download",
                done: n,
                total: count,
                file: t.file.path.clone(),
                bytes: bytes.load(Ordering::Relaxed),
                total_bytes,
            });
            Ok::<_, anyhow::Error>((t.file.path, t.keep, sha))
        }
    });
    let results: Vec<(String, bool, String)> = stream::iter(jobs)
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .try_collect()
        .await?;
    for (path, keep, sha) in results {
        if !keep {
            managed.insert(path, sha);
        }
    }

    // 3) Aus dem Pack entfernte Dateien löschen (nur, was wir selbst verwaltet haben)
    let mut removed = 0;
    for rel in &removals {
        let Ok(rel_path) = relative_path(rel) else {
            continue;
        };
        let target = game.join(rel_path);
        match tokio::fs::remove_file(&target).await {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(explain_fs_error(e, &target)),
        }
    }

    let state = json!({
        "packId": manifest.id,
        "version": manifest.version,
        "source": source,
        "updatedAt": crate::now_millis(),
        "files": managed,
    });
    tokio::fs::write(&state_path, serde_json::to_string_pretty(&state)?).await?;
    progress("done", count, count, "", total_bytes, total_bytes);

    Ok(SyncResult {
        instance_id: manifest.instance_id,
        version: manifest.version,
        server: manifest.server,
        downloaded: count,
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fester Name je Test: der nächste Lauf räumt ihn auf, statt neue Ordner anzulegen.
    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mylauncher-pack-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(game: &Path, rel: &str, text: &str) -> String {
        let path = game.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        hex::encode(Sha1::digest(text.as_bytes()))
    }

    fn file(path: &str, sha1: &str) -> PackFile {
        PackFile {
            path: path.into(),
            url: path.into(),
            sha1: Some(sha1.into()),
            size: Some(10),
            overwrite: None,
        }
    }

    fn manifest(files: Vec<PackFile>) -> Manifest {
        Manifest {
            format_version: 1,
            id: "test".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            instance_id: "Test".into(),
            minecraft: "1.21.4".into(),
            loader: LoaderSpec::default(),
            server: String::new(),
            memory: None,
            files,
        }
    }

    async fn plan(game: &Path, m: &Manifest, old: &[(&str, &str)]) -> Work {
        let old: BTreeMap<String, String> =
            old.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect();
        examine(game, m, &old, &|_, _, _| {}).await.unwrap()
    }

    fn kind(work: &Work, path: &str) -> Option<ChangeKind> {
        work.changes.iter().find(|c| c.path == path).map(|c| c.kind)
    }

    #[tokio::test]
    async fn neue_datei_ist_ein_zugang() {
        let game = tmp("add");
        let m = manifest(vec![file("mods/neu.jar", "0".repeat(40).as_str())]);
        let work = plan(&game, &m, &[]).await;
        assert_eq!(kind(&work, "mods/neu.jar"), Some(ChangeKind::Add));
        assert_eq!(work.todo.len(), 1);
        assert_eq!(work.unchanged, 0);
    }

    #[tokio::test]
    async fn passende_datei_bleibt_unberuehrt() {
        let game = tmp("same");
        let sha = write(&game, "mods/gut.jar", "inhalt");
        let work = plan(&game, &manifest(vec![file("mods/gut.jar", &sha)]), &[]).await;
        assert!(work.changes.is_empty());
        assert_eq!(work.unchanged, 1);
        assert_eq!(work.managed.get("mods/gut.jar"), Some(&sha));
    }

    /// Lokal verändert, aber das Pack hat die Datei nicht angerührt -> Reparatur.
    #[tokio::test]
    async fn lokale_abweichung_ist_eine_reparatur() {
        let game = tmp("repair");
        write(&game, "mods/kaputt.jar", "beschaedigt");
        let soll = hex::encode(Sha1::digest(b"original"));
        let work = plan(&game, &manifest(vec![file("mods/kaputt.jar", &soll)]), &[("mods/kaputt.jar", &soll)]).await;
        assert_eq!(kind(&work, "mods/kaputt.jar"), Some(ChangeKind::Repair));
    }

    /// Gelöschte Datei, die wir installiert hatten, ist ebenfalls eine Reparatur.
    #[tokio::test]
    async fn geloeschte_datei_wird_repariert() {
        let game = tmp("deleted");
        let soll = hex::encode(Sha1::digest(b"original"));
        let work = plan(&game, &manifest(vec![file("mods/weg.jar", &soll)]), &[("mods/weg.jar", &soll)]).await;
        assert_eq!(kind(&work, "mods/weg.jar"), Some(ChangeKind::Repair));
    }

    /// Das Pack liefert eine andere Fassung als beim letzten Mal -> Update.
    #[tokio::test]
    async fn neue_fassung_im_pack_ist_ein_update() {
        let game = tmp("update");
        write(&game, "mods/alt.jar", "alte fassung");
        let alt = hex::encode(Sha1::digest(b"alte fassung"));
        let neu = hex::encode(Sha1::digest(b"neue fassung"));
        let work = plan(&game, &manifest(vec![file("mods/alt.jar", &neu)]), &[("mods/alt.jar", &alt)]).await;
        assert_eq!(kind(&work, "mods/alt.jar"), Some(ChangeKind::Update));
    }

    #[tokio::test]
    async fn aus_dem_pack_entfernte_datei_wird_geloescht() {
        let game = tmp("remove");
        let sha = write(&game, "mods/raus.jar", "egal");
        let work = plan(&game, &manifest(vec![]), &[("mods/raus.jar", &sha)]).await;
        assert_eq!(kind(&work, "mods/raus.jar"), Some(ChangeKind::Remove));
        assert_eq!(work.removals, vec!["mods/raus.jar".to_string()]);
    }

    /// Fehlt die Datei schon auf der Platte, wird sie nicht als Änderung angezeigt,
    /// bleibt aber in der Löschliste (Zustandsdatei muss sie loswerden).
    #[tokio::test]
    async fn bereits_fehlende_loeschung_wird_nicht_angezeigt() {
        let game = tmp("remove-missing");
        let work = plan(&game, &manifest(vec![]), &[("mods/schon-weg.jar", &"0".repeat(40))]).await;
        assert!(work.changes.is_empty());
        assert_eq!(work.removals.len(), 1);
    }

    #[tokio::test]
    async fn vorgabedatei_bleibt_dem_spieler() {
        let game = tmp("keep");
        write(&game, "options.txt", "eigene einstellungen");
        let mut f = file("options.txt", &"0".repeat(40));
        f.overwrite = Some(false);
        let work = plan(&game, &manifest(vec![f]), &[]).await;
        assert!(work.changes.is_empty(), "Vorgabedatei darf nicht überschrieben werden");
        assert!(work.managed.is_empty(), "und gehört nicht in die Verwaltung");
        assert_eq!(work.unchanged, 1);
    }

    #[test]
    fn fremde_mods_werden_erkannt() {
        let game = tmp("extras");
        write(&game, "mods/pack-mod.jar", "a");
        write(&game, "mods/selbst-installiert.jar", "bb");
        write(&game, "mods/.index/pack.json", "c");
        write(&game, "mods/unten/tief.jar", "d");
        write(&game, "config/egal.toml", "e");
        let managed = HashSet::from(["mods/pack-mod.jar".to_string()]);
        let (extras, truncated) = scan_extras(&game, &managed);
        let paths: Vec<&str> = extras.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["mods/selbst-installiert.jar", "mods/unten/tief.jar"]);
        assert_eq!(extras[0].size, 2);
        assert!(!truncated);
    }
}
