//! Was der Launcher abruft – ohne Anmeldung, unter `/launcher`.
//!
//! ```text
//! /launcher/config.json                           Packs, News, Server, Branding
//! /launcher/packs/<id>/manifest.json              ausgelieferte Version eines Packs
//! /launcher/packs/<id>/files/<sha1>/<name>        Dateien (relativ zum Manifest)
//! /launcher/update.json                           Launcher-Update
//! /launcher/updates/<version>/<installer.exe>
//! ```

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use protocol::release::Feed;
use serde_json::Value;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::store::{self, url_segment};
use crate::{internal, public_base, ApiError, Shared};

fn missing(what: &str) -> ApiError {
    (StatusCode::NOT_FOUND, format!("{what} nicht gefunden"))
}

/// Antworten, die sich jederzeit ändern können, nie zwischenspeichern.
fn fresh(json: impl IntoResponse) -> Response {
    let mut res = json.into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

async fn config(State(state): State<Shared>, headers: HeaderMap) -> Response {
    // Ist dieser Server auch Wegpunkt-Server, schlägt er sich selbst vor.
    let waypoint = if state.tokens.is_empty() {
        String::new()
    } else {
        public_base(&state, &headers)
    };
    let value: Value = state.store.lock().await.launcher_config(&waypoint);
    fresh(Json(value))
}

async fn manifest(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let store = state.store.lock().await;
    let version = store
        .packs
        .get(&id)
        .and_then(|p| p.current.clone())
        .ok_or_else(|| missing("Pack"))?;
    let manifest = store
        .load_manifest(&id, &version)
        .map_err(internal)?
        .ok_or_else(|| missing("Manifest"))?;
    Ok(fresh(Json(manifest)))
}

async fn send_file(path: PathBuf, req: Request) -> Response {
    match ServeFile::new(path).oneshot(req).await {
        Ok(res) => res.map(Body::new).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn pack_file(
    State(state): State<Shared>,
    Path((_id, sha1, _name)): Path<(String, String, String)>,
    req: Request,
) -> Result<Response, ApiError> {
    if !store::valid_sha1(&sha1) {
        return Err(missing("Datei"));
    }
    let path = state.store.lock().await.blob_path(&sha1);
    if !path.is_file() {
        return Err(missing("Datei"));
    }
    let mut res = send_file(path, req).await;
    // Inhalt hängt an der SHA1 in der URL – darf ewig im Cache bleiben.
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(res)
}

async fn update_feed(
    State(state): State<Shared>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store = state.store.lock().await;
    let release = store
        .releases
        .current
        .as_ref()
        .and_then(|v| store.releases.list.iter().find(|r| &r.version == v))
        .ok_or_else(|| missing("Update"))?;
    // Absolute URL: ältere Launcher lösen relative Installer-Adressen nicht auf.
    let feed = Feed {
        version: release.version.clone(),
        notes: release.notes.clone(),
        pub_date: release.pub_date.clone(),
        url: format!(
            "{}/launcher/updates/{}/{}",
            public_base(&state, &headers),
            url_segment(&release.version),
            url_segment(&release.file)
        ),
        sha256: release.sha256.clone(),
        size: Some(release.size),
    };
    Ok(fresh(Json(feed)))
}

async fn installer(
    State(state): State<Shared>,
    Path((version, file)): Path<(String, String)>,
    req: Request,
) -> Result<Response, ApiError> {
    let path = {
        let store = state.store.lock().await;
        let release = store
            .releases
            .list
            .iter()
            .find(|r| r.version == version && r.file == file)
            .ok_or_else(|| missing("Installer"))?;
        store.release_path(release)
    };
    let mut res = send_file(path, req).await;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.microsoft.portable-executable"),
    );
    Ok(res)
}

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/config.json", get(config))
        .route("/packs/{id}/manifest.json", get(manifest))
        .route("/packs/{id}/files/{sha1}/{name}", get(pack_file))
        .route("/update.json", get(update_feed))
        .route("/updates/{version}/{file}", get(installer))
}
