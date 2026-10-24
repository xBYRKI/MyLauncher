//! Admin-Oberfläche und ihre API unter `/admin`.
//!
//! Anmeldung mit dem Admin-Passwort. Der Browser bekommt ein Sitzungs-Cookie
//! (`HttpOnly`, `SameSite=Strict`) und schickt zusätzlich `X-MyLauncher: 1` mit –
//! diesen Header kann eine fremde Seite nicht setzen. Skripte nehmen stattdessen
//! `Authorization: Bearer <Admin-Passwort>`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::store::{self, DraftFile, Pack, PackMeta, Release, Site, Store};
use crate::{bad, internal, now_millis, public_base, ApiError, Shared};

const COOKIE: &str = "mls_admin";
const SESSION_MS: u64 = 12 * 60 * 60 * 1000;
/// Obergrenze je hochgeladener Datei.
const MAX_UPLOAD: u64 = 2 * 1024 * 1024 * 1024;

const INDEX_HTML: &str = include_str!("admin/index.html");
const APP_JS: &str = include_str!("admin/app.js");
const APP_CSS: &str = include_str!("admin/app.css");
const FONT: &[u8] = include_bytes!("../../../app/assets/Monocraft.ttf");
const LOGO: &[u8] = include_bytes!("../../../app/assets/logo.png");

// ---------- Anmeldung ----------

pub struct Auth {
    password: String,
    /// Sitzungs-Token → läuft ab (ms)
    sessions: Mutex<HashMap<String, u64>>,
    /// Anmeldeversuche nacheinander, mit Pause nach Fehlversuch
    login_lock: Mutex<()>,
}

impl Auth {
    pub fn new(password: String) -> Self {
        Self {
            password,
            sessions: Mutex::new(HashMap::new()),
            login_lock: Mutex::new(()),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.password.is_empty()
    }

    async fn check(&self, headers: &HeaderMap) -> bool {
        if !self.enabled() {
            return false;
        }
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if let Some(token) = bearer {
            return same(token.as_bytes(), self.password.as_bytes());
        }
        let csrf_ok = headers.get("x-mylauncher").is_some_and(|v| v == "1");
        let Some(token) = cookie(headers, COOKIE) else {
            return false;
        };
        let mut sessions = self.sessions.lock().await;
        let now = now_millis();
        sessions.retain(|_, until| *until > now);
        csrf_ok && sessions.contains_key(&token)
    }
}

/// Vergleich in konstanter Zeit (Passwort).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// Extraktor: Handler mit `_: Admin` laufen nur für angemeldete Admins.
pub struct Admin;

impl FromRequestParts<Shared> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, ApiError> {
        if state.auth.check(&parts.headers).await {
            Ok(Admin)
        } else {
            Err((StatusCode::UNAUTHORIZED, "Nicht angemeldet".into()))
        }
    }
}

#[derive(Deserialize)]
struct Login {
    password: String,
}

async fn login(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<Login>,
) -> Result<Response, ApiError> {
    if !state.auth.enabled() {
        return Err((
            StatusCode::FORBIDDEN,
            "Admin-Zugang ist aus – Server mit --admin-password starten".into(),
        ));
    }
    let _guard = state.auth.login_lock.lock().await;
    if !same(req.password.as_bytes(), state.auth.password.as_bytes()) {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        return Err((StatusCode::UNAUTHORIZED, "Falsches Passwort".into()));
    }
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    state
        .auth
        .sessions
        .lock()
        .await
        .insert(token.clone(), now_millis() + SESSION_MS);
    let secure = if public_base(&state, &headers).starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{COOKIE}={token}; Path=/admin; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        SESSION_MS / 1000
    );
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true })),
    )
        .into_response())
}

async fn logout(State(state): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers, COOKIE) {
        state.auth.sessions.lock().await.remove(&token);
    }
    (
        [(
            header::SET_COOKIE,
            format!("{COOKIE}=; Path=/admin; HttpOnly; SameSite=Strict; Max-Age=0"),
        )],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

// ---------- Übersicht ----------

async fn status(_: Admin, State(state): State<Shared>, headers: HeaderMap) -> Json<Value> {
    let base = public_base(&state, &headers);
    let store = state.store.lock().await;
    let (blobs, bytes) = store.blob_usage();
    let mut teams: Vec<&String> = state.tokens.values().collect();
    teams.sort();
    teams.dedup();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "publicUrl": base,
        "configUrl": format!("{base}/launcher/config.json"),
        "updateUrl": format!("{base}/launcher/update.json"),
        "teams": teams,
        "blobs": { "count": blobs, "bytes": bytes },
    }))
}

async fn gc(_: Admin, State(state): State<Shared>) -> Result<Json<Value>, ApiError> {
    let store = state.store.lock().await;
    let (count, bytes) = store.collect_garbage().map_err(internal)?;
    Ok(Json(json!({ "removed": count, "bytes": bytes })))
}

// ---------- Seite: News, Server, Branding ----------

async fn get_site(_: Admin, State(state): State<Shared>) -> Json<Site> {
    Json(state.store.lock().await.site.clone())
}

async fn put_site(
    _: Admin,
    State(state): State<Shared>,
    Json(mut site): Json<Site>,
) -> Result<Json<Site>, ApiError> {
    for s in &site.servers {
        if s.address.trim().is_empty() {
            return Err(bad("Jeder Server braucht eine Adresse"));
        }
    }
    for n in &site.news {
        if n.title.trim().is_empty() {
            return Err(bad("Jede News braucht einen Titel"));
        }
        for link in [&n.image, &n.url] {
            if !link.is_empty() && !link.starts_with("https://") && !link.starts_with("http://") {
                return Err(bad("Bild- und Link-Adressen müssen mit http(s):// beginnen"));
            }
        }
    }
    let accent = site.brand.accent.trim();
    if !accent.is_empty()
        && !(accent.len() == 7
            && accent.starts_with('#')
            && accent[1..].chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(bad("Akzentfarbe als #RRGGBB angeben"));
    }
    site.brand.accent = accent.to_string();
    let mut store = state.store.lock().await;
    store.site = site;
    store.save_site().map_err(internal)?;
    Ok(Json(store.site.clone()))
}

// ---------- Modpacks ----------

/// Was sich im Entwurf gegenüber der aktuellen Version geändert hat.
fn draft_diff(store: &Store, pack: &Pack) -> Value {
    let Some(current) = &pack.current else {
        return json!({ "unpublished": true, "dirty": true,
            "added": pack.files.len(), "changed": 0, "removed": 0, "meta": true });
    };
    let Ok(Some(old)) = store.load_manifest(&pack.id, current) else {
        return json!({ "dirty": true, "error": "Manifest der aktuellen Version fehlt" });
    };
    let old_files = store::draft_files(&old);
    let added = pack.files.keys().filter(|p| !old_files.contains_key(*p)).count();
    let removed = old_files.keys().filter(|p| !pack.files.contains_key(*p)).count();
    let changed = pack
        .files
        .iter()
        .filter(|(p, f)| old_files.get(*p).is_some_and(|o| o != *f))
        .count();
    let mut draft = Store::draft_manifest(pack, current);
    let mut old = old;
    draft.files.clear();
    old.files.clear();
    let meta = serde_json::to_value(&draft).ok() != serde_json::to_value(&old).ok();
    json!({
        "dirty": meta || added + removed + changed > 0,
        "added": added, "changed": changed, "removed": removed, "meta": meta,
    })
}

fn pack_summary(store: &Store, pack: &Pack) -> Value {
    json!({
        "id": pack.id,
        "name": pack.meta.name,
        "minecraft": pack.meta.minecraft,
        "loader": pack.meta.loader,
        "listed": pack.meta.listed,
        "current": pack.current,
        "fileCount": pack.files.len(),
        "totalSize": pack.files.values().map(|f| f.size).sum::<u64>(),
        "updatedAt": pack.updated_at,
        "draft": draft_diff(store, pack),
    })
}

fn pack_detail(store: &Store, pack: &Pack) -> Value {
    // Stand der ausgelieferten Version – zum Markieren neuer und geänderter Dateien.
    let old = pack
        .current
        .as_ref()
        .and_then(|v| store.load_manifest(&pack.id, v).ok().flatten())
        .map(|m| store::draft_files(&m));
    let mut out = serde_json::to_value(pack).unwrap_or_default();
    out["draft"] = draft_diff(store, pack);
    out["files"] = pack
        .files
        .iter()
        .map(|(path, f)| {
            let status = match old.as_ref().map(|o| o.get(path)) {
                None => "",
                Some(None) => "new",
                Some(Some(o)) if o != f => "changed",
                Some(Some(_)) => "",
            };
            json!({ "path": path, "sha1": f.sha1, "size": f.size, "overwrite": f.overwrite, "status": status })
        })
        .collect::<Vec<_>>()
        .into();
    out["removedFiles"] = old
        .map(|o| {
            o.into_keys()
                .filter(|p| !pack.files.contains_key(p))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into();
    out
}

fn normalize(mut meta: PackMeta) -> Result<PackMeta, ApiError> {
    meta.name = meta.name.trim().into();
    meta.description = meta.description.trim().into();
    meta.minecraft = meta.minecraft.trim().into();
    meta.server = meta.server.trim().into();
    meta.instance_id = meta.instance_id.trim().into();
    meta.loader.kind = meta.loader.kind.trim().to_lowercase();
    meta.loader.version = meta.loader.version.trim().into();
    if meta.loader.kind == "vanilla" {
        meta.loader.kind.clear();
    }
    if meta.loader.kind.is_empty() {
        meta.loader.version.clear();
    }
    if meta
        .instance_id
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control())
    {
        return Err(bad("Instanz-Ordner enthält ungültige Zeichen"));
    }
    store::check_meta(&meta).map_err(bad)?;
    Ok(meta)
}

async fn list_packs(_: Admin, State(state): State<Shared>) -> Json<Vec<Value>> {
    let store = state.store.lock().await;
    Json(store.packs.values().map(|p| pack_summary(&store, p)).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewPack {
    id: String,
    #[serde(flatten)]
    meta: PackMeta,
    /// Dateien und Einstellungen eines vorhandenen Packs übernehmen.
    #[serde(default)]
    copy_from: Option<String>,
}

async fn create_pack(
    _: Admin,
    State(state): State<Shared>,
    Json(req): Json<NewPack>,
) -> Result<Json<Value>, ApiError> {
    let id = req.id.trim().to_string();
    if !store::valid_pack_id(&id) {
        return Err(bad("ID: nur Kleinbuchstaben, Ziffern, - und _ (max. 64 Zeichen)"));
    }
    let mut store = state.store.lock().await;
    if store.packs.contains_key(&id) {
        return Err(bad(format!("Ein Pack mit der ID „{id}“ gibt es schon")));
    }
    let (meta, files) = match req.copy_from.as_deref().filter(|s| !s.is_empty()) {
        Some(src) => {
            let src = store.packs.get(src).ok_or_else(|| bad("Vorlage nicht gefunden"))?;
            let mut meta = src.meta.clone();
            meta.name = if req.meta.name.trim().is_empty() {
                format!("{} (Kopie)", meta.name)
            } else {
                req.meta.name.clone()
            };
            meta.listed = false;
            meta.instance_id.clear();
            (meta, src.files.clone())
        }
        None => (req.meta, BTreeMap::new()),
    };
    let meta = normalize(meta)?;
    store.packs.insert(
        id.clone(),
        Pack {
            id: id.clone(),
            meta,
            files,
            versions: vec![],
            current: None,
            updated_at: now_millis(),
        },
    );
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(pack_detail(&store, &store.packs[&id])))
}

async fn get_pack(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = state.store.lock().await;
    let pack = store.packs.get(&id).ok_or_else(not_found)?;
    Ok(Json(pack_detail(&store, pack)))
}

fn not_found() -> ApiError {
    (StatusCode::NOT_FOUND, "Pack nicht gefunden".into())
}

async fn put_pack(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(meta): Json<PackMeta>,
) -> Result<Json<Value>, ApiError> {
    let meta = normalize(meta)?;
    let mut store = state.store.lock().await;
    // „Im Launcher zeigen“ greift erst, sobald eine Version veröffentlicht ist.
    let pack = store.packs.get_mut(&id).ok_or_else(not_found)?;
    pack.meta = meta;
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(pack_detail(&store, &store.packs[&id])))
}

async fn delete_pack(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    store.delete_pack(&id).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

// ----- Dateien -----

struct Received {
    tmp: PathBuf,
    sha1: String,
    sha256: String,
    size: u64,
    head: Vec<u8>,
}

/// Anfrage-Body in eine temporäre Datei streamen und dabei prüfen/hashen.
async fn receive(body: Body, dir: &FsPath) -> Result<Received, ApiError> {
    let tmp = dir.join(uuid::Uuid::new_v4().simple().to_string());
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut size = 0u64;
    let mut head = Vec::new();
    let result: Result<(), ApiError> = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(internal)?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| bad(format!("Upload abgebrochen: {e}")))?;
            size += chunk.len() as u64;
            if size > MAX_UPLOAD {
                return Err((StatusCode::PAYLOAD_TOO_LARGE, "Datei zu groß".into()));
            }
            if head.len() < 4 {
                head.extend(chunk.iter().take(4 - head.len()));
            }
            sha1.update(&chunk);
            sha256.update(&chunk);
            file.write_all(&chunk).await.map_err(internal)?;
        }
        file.flush().await.map_err(internal)?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(Received {
        tmp,
        sha1: hex::encode(sha1.finalize()),
        sha256: hex::encode(sha256.finalize()),
        size,
        head,
    })
}

/// `mods` als Datei und `mods/x.jar` gleichzeitig geht im Dateisystem nicht.
fn path_conflict(files: &BTreeMap<String, DraftFile>, path: &str) -> Option<String> {
    let as_dir = format!("{path}/");
    if let Some(other) = files.keys().find(|k| k.starts_with(&as_dir)) {
        return Some(other.clone());
    }
    let mut prefix = String::new();
    for part in path.split('/').collect::<Vec<_>>().split_last()?.1 {
        prefix.push_str(part);
        if files.contains_key(&prefix) {
            return Some(prefix);
        }
        prefix.push('/');
    }
    None
}

#[derive(Deserialize)]
struct UploadQuery {
    path: String,
    #[serde(default)]
    overwrite: Option<bool>,
}

async fn upload_file(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    let path = store::clean_file_path(&q.path).map_err(bad)?;
    let tmp_dir = {
        let store = state.store.lock().await;
        let pack = store.packs.get(&id).ok_or_else(not_found)?;
        if let Some(other) = path_conflict(&pack.files, &path) {
            return Err(bad(format!("{path} kollidiert mit {other}")));
        }
        store.tmp_dir()
    };
    let got = receive(body, &tmp_dir).await?;
    let mut store = state.store.lock().await;
    let Some(pack) = store.packs.get(&id) else {
        let _ = std::fs::remove_file(&got.tmp);
        return Err(not_found());
    };
    let overwrite = q
        .overwrite
        .unwrap_or_else(|| pack.files.get(&path).map(|f| f.overwrite).unwrap_or(true));
    store.adopt_blob(&got.tmp, &got.sha1).map_err(internal)?;
    let pack = store.packs.get_mut(&id).expect("geprüft");
    pack.files.insert(
        path.clone(),
        DraftFile {
            sha1: got.sha1.clone(),
            size: got.size,
            overwrite,
        },
    );
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(json!({ "path": path, "sha1": got.sha1, "size": got.size })))
}

#[derive(Deserialize)]
struct Paths {
    paths: Vec<String>,
    #[serde(default)]
    overwrite: Option<bool>,
}

async fn delete_files(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<Paths>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    let pack = store.packs.get_mut(&id).ok_or_else(not_found)?;
    let before = pack.files.len();
    for p in &req.paths {
        pack.files.remove(p);
    }
    let removed = before - pack.files.len();
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(json!({ "removed": removed })))
}

async fn set_overwrite(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<Paths>,
) -> Result<Json<Value>, ApiError> {
    let overwrite = req.overwrite.ok_or_else(|| bad("overwrite fehlt"))?;
    let mut store = state.store.lock().await;
    let pack = store.packs.get_mut(&id).ok_or_else(not_found)?;
    for p in &req.paths {
        if let Some(f) = pack.files.get_mut(p) {
            f.overwrite = overwrite;
        }
    }
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct Move {
    from: String,
    to: String,
}

/// Datei umbenennen oder ganzen Ordner verschieben.
async fn move_files(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<Move>,
) -> Result<Json<Value>, ApiError> {
    let from = store::clean_file_path(&req.from).map_err(bad)?;
    let to = store::clean_file_path(&req.to).map_err(bad)?;
    let mut store = state.store.lock().await;
    let pack = store.packs.get_mut(&id).ok_or_else(not_found)?;
    let moves: Vec<(String, String)> = if pack.files.contains_key(&from) {
        vec![(from.clone(), to.clone())]
    } else {
        let dir = format!("{from}/");
        pack.files
            .keys()
            .filter_map(|k| k.strip_prefix(&dir).map(|rest| (k.clone(), format!("{to}/{rest}"))))
            .collect()
    };
    if moves.is_empty() {
        return Err(bad(format!("{from} gibt es nicht")));
    }
    let mut files = pack.files.clone();
    let moved: Vec<(String, DraftFile)> = moves
        .iter()
        .map(|(old, new)| (new.clone(), files.remove(old).expect("vorhanden")))
        .collect();
    for (new, file) in moved {
        store::clean_file_path(&new).map_err(bad)?;
        if files.contains_key(&new) {
            return Err(bad(format!("{new} gibt es schon")));
        }
        if let Some(other) = path_conflict(&files, &new) {
            return Err(bad(format!("{new} kollidiert mit {other}")));
        }
        files.insert(new, file);
    }
    pack.files = files;
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(json!({ "moved": moves.len() })))
}

// ----- Versionen -----

#[derive(Deserialize)]
struct Publish {
    version: String,
    #[serde(default)]
    notes: String,
}

async fn publish(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<Publish>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    store
        .publish(&id, req.version.trim(), &req.notes)
        .map_err(bad)?;
    println!("[packs] {id} {} veröffentlicht", req.version.trim());
    Ok(Json(pack_detail(&store, &store.packs[&id])))
}

#[derive(Deserialize)]
struct VersionReq {
    version: String,
}

/// Andere Version ausliefern (Rollback).
async fn set_current(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<VersionReq>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    let pack = store.packs.get_mut(&id).ok_or_else(not_found)?;
    if !pack.versions.iter().any(|v| v.version == req.version) {
        return Err(bad("Version nicht gefunden"));
    }
    pack.current = Some(req.version.clone());
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(pack_detail(&store, &store.packs[&id])))
}

async fn reset_draft(
    _: Admin,
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<VersionReq>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    store.reset_draft(&id, &req.version).map_err(bad)?;
    Ok(Json(pack_detail(&store, &store.packs[&id])))
}

async fn delete_version(
    _: Admin,
    State(state): State<Shared>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    let pack = store.packs.get_mut(&id).ok_or_else(not_found)?;
    if pack.current.as_deref() == Some(version.as_str()) {
        return Err(bad("Die ausgelieferte Version kann nicht gelöscht werden"));
    }
    let before = pack.versions.len();
    pack.versions.retain(|v| v.version != version);
    if pack.versions.len() == before {
        return Err(bad("Version nicht gefunden"));
    }
    let path = store.manifest_path(&id, &version);
    let _ = std::fs::remove_file(path);
    store.save_pack(&id).map_err(internal)?;
    Ok(Json(pack_detail(&store, &store.packs[&id])))
}

// ---------- Launcher-Updates ----------

fn releases_json(store: &Store) -> Value {
    let mut list = store.releases.list.clone();
    list.sort_by(|a, b| protocol::release::compare_versions(&b.version, &a.version));
    json!({ "list": list, "current": store.releases.current })
}

async fn list_releases(_: Admin, State(state): State<Shared>) -> Json<Value> {
    Json(releases_json(&*state.store.lock().await))
}

#[derive(Deserialize)]
struct ReleaseQuery {
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    file: String,
    /// Gleich als aktuelles Update anbieten.
    #[serde(default)]
    activate: bool,
}

async fn upload_release(
    _: Admin,
    State(state): State<Shared>,
    Query(q): Query<ReleaseQuery>,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    let version = q.version.trim().trim_start_matches(['v', 'V']).to_string();
    if !store::valid_version(&version) {
        return Err(bad("Ungültige Version"));
    }
    let tmp_dir = {
        let store = state.store.lock().await;
        if store.releases.list.iter().any(|r| r.version == version) {
            return Err(bad(format!("Version {version} gibt es schon")));
        }
        store.tmp_dir()
    };
    let got = receive(body, &tmp_dir).await?;
    if !got.head.starts_with(b"MZ") {
        let _ = std::fs::remove_file(&got.tmp);
        return Err(bad("Das ist keine Windows-Exe"));
    }
    let mut store = state.store.lock().await;
    if store.releases.list.iter().any(|r| r.version == version) {
        let _ = std::fs::remove_file(&got.tmp);
        return Err(bad(format!("Version {version} gibt es schon")));
    }
    let release = Release {
        file: store::clean_installer_name(&q.file, &version),
        version: version.clone(),
        notes: q.notes.trim().to_string(),
        pub_date: today(),
        sha256: got.sha256,
        size: got.size,
        uploaded_at: now_millis(),
    };
    let dest = store.release_path(&release);
    let moved = std::fs::create_dir_all(dest.parent().expect("Ordner"))
        .and_then(|_| std::fs::rename(&got.tmp, &dest));
    if let Err(e) = moved {
        let _ = std::fs::remove_file(&got.tmp);
        return Err(internal(e));
    }
    store.releases.list.push(release);
    if q.activate {
        store.releases.current = Some(version.clone());
    }
    store.save_releases().map_err(internal)?;
    println!("[update] Launcher {version} hochgeladen");
    Ok(Json(releases_json(&store)))
}

#[derive(Deserialize)]
struct CurrentRelease {
    version: Option<String>,
}

async fn set_current_release(
    _: Admin,
    State(state): State<Shared>,
    Json(req): Json<CurrentRelease>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    if let Some(v) = &req.version {
        if !store.releases.list.iter().any(|r| &r.version == v) {
            return Err(bad("Version nicht gefunden"));
        }
    }
    store.releases.current = req.version;
    store.save_releases().map_err(internal)?;
    Ok(Json(releases_json(&store)))
}

#[derive(Deserialize)]
struct Notes {
    notes: String,
}

async fn set_release_notes(
    _: Admin,
    State(state): State<Shared>,
    Path(version): Path<String>,
    Json(req): Json<Notes>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    let r = store
        .releases
        .list
        .iter_mut()
        .find(|r| r.version == version)
        .ok_or_else(|| bad("Version nicht gefunden"))?;
    r.notes = req.notes.trim().to_string();
    store.save_releases().map_err(internal)?;
    Ok(Json(releases_json(&store)))
}

async fn delete_release(
    _: Admin,
    State(state): State<Shared>,
    Path(version): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state.store.lock().await;
    if store.releases.current.as_deref() == Some(version.as_str()) {
        return Err(bad("Die angebotene Version kann nicht gelöscht werden"));
    }
    let Some(pos) = store.releases.list.iter().position(|r| r.version == version) else {
        return Err(bad("Version nicht gefunden"));
    };
    let release = store.releases.list.remove(pos);
    let path = store.release_path(&release);
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    store.save_releases().map_err(internal)?;
    Ok(Json(releases_json(&store)))
}

/// Heutiges Datum (UTC) als `YYYY-MM-DD`.
fn today() -> String {
    let days = (now_millis() / 86_400_000) as i64;
    // Howard Hinnant, „civil_from_days“
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

// ---------- Oberfläche ----------

fn asset(content_type: &'static str, body: impl IntoResponse) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'self'; img-src 'self' https: data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
                ),
            ),
            (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
            (header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
        ],
        body,
    )
        .into_response()
}

pub async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML)
}

async fn no_store(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

pub fn router() -> Router<Shared> {
    let api = Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/status", get(status))
        .route("/gc", post(gc))
        .route("/site", get(get_site).put(put_site))
        .route("/packs", get(list_packs).post(create_pack))
        .route("/packs/{id}", get(get_pack).put(put_pack).delete(delete_pack))
        .route("/packs/{id}/files", put(upload_file))
        .route("/packs/{id}/files/delete", post(delete_files))
        .route("/packs/{id}/files/overwrite", post(set_overwrite))
        .route("/packs/{id}/files/move", post(move_files))
        .route("/packs/{id}/publish", post(publish))
        .route("/packs/{id}/current", post(set_current))
        .route("/packs/{id}/reset", post(reset_draft))
        .route("/packs/{id}/versions/{version}", delete(delete_version))
        .route("/releases", get(list_releases).put(upload_release))
        .route("/releases/current", post(set_current_release))
        .route(
            "/releases/{version}",
            delete(delete_release).put(set_release_notes),
        )
        .layer(axum::middleware::map_response(no_store));
    Router::new()
        .route("/app.js", get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }))
        .route("/app.css", get(|| async { asset("text/css; charset=utf-8", APP_CSS) }))
        .route("/font.ttf", get(|| async { asset("font/ttf", FONT) }))
        .route("/logo.png", get(|| async { asset("image/png", LOGO) }))
        .nest("/api", api)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datum_stimmt() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
    }

    #[test]
    fn ordner_und_datei_kollidieren() {
        let mut files = BTreeMap::new();
        let f = DraftFile {
            sha1: "0".repeat(40),
            size: 1,
            overwrite: true,
        };
        files.insert("mods/a.jar".to_string(), f.clone());
        files.insert("config".to_string(), f);
        assert_eq!(path_conflict(&files, "mods"), Some("mods/a.jar".into()));
        assert_eq!(path_conflict(&files, "config/x.toml"), Some("config".into()));
        assert_eq!(path_conflict(&files, "mods/b.jar"), None);
    }

    #[test]
    fn cookies_werden_gelesen() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; mls_admin=abc; b=2"));
        assert_eq!(cookie(&h, COOKIE).as_deref(), Some("abc"));
        assert!(same(b"geheim", b"geheim"));
        assert!(!same(b"geheim", b"geheiM"));
        assert!(!same(b"geheim", b"geheim2"));
    }
}
