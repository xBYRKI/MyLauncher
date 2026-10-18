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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::ini::Ini;
use crate::prism::{self, Prism};

pub const STATE_FILE: &str = "mylauncher-pack.json";
const PARALLEL_DOWNLOADS: usize = 4;

pub fn loader_uid(kind: &str) -> Option<&'static str> {
    match kind {
        "fabric" => Some("net.fabricmc.fabric-loader"),
        "quilt" => Some("org.quiltmc.quilt-loader"),
        "forge" => Some("net.minecraftforge"),
        "neoforge" => Some("net.neoforged"),
        _ => None,
    }
}

// ---------- Manifest ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderSpec {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Memory {
    #[serde(default)]
    pub min: Option<u32>,
    pub max: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// `false` = nur anlegen, wenn die Datei fehlt (z. B. options.txt)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default = "one")]
    pub format_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance_id: String,
    pub minecraft: String,
    #[serde(default)]
    pub loader: LoaderSpec,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<Memory>,
    #[serde(default)]
    pub files: Vec<PackFile>,
}

fn one() -> u32 {
    1
}

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

    // 1) Prüfen, welche Dateien fehlen oder veraltet sind
    let mut todo = vec![];
    let mut managed = BTreeMap::new();
    let total = manifest.files.len();
    for (i, f) in manifest.files.iter().enumerate() {
        progress("check", i, total, &f.path, 0, 0);
        let target = game.join(relative_path(&f.path)?);
        let keep = f.overwrite == Some(false);
        if target.exists() {
            if keep {
                continue;
            }
            if let Some(expected) = &f.sha1 {
                if sha1_file(&target).await?.eq_ignore_ascii_case(expected) {
                    managed.insert(f.path.clone(), expected.to_lowercase());
                    continue;
                }
            }
        }
        todo.push(Todo {
            file: f.clone(),
            target,
            keep,
        });
    }

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
    let current: HashSet<&String> = managed.keys().collect();
    let mut removed = 0;
    for rel in old_files.keys().filter(|r| !current.contains(r)) {
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
