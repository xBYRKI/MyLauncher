//! Was der Launcher abruft – ohne Anmeldung, unter `/launcher`.
//! Dazu die Download-Seite für Spieler unter `/` und `/download`.
//!
//! ```text
//! /                                               Download-Seite
//! /download                                       → aktueller Installer
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
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use protocol::release::Feed;
use serde_json::Value;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::store::{self, Store};
use crate::{admin, internal, public_base, ApiError, Shared};

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

async fn config(State(state): State<Shared>) -> Response {
    let value: Value = state.store.lock().await.launcher_config();
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
    let release = store.current_release().ok_or_else(|| missing("Update"))?;
    // Absolute URL: ältere Launcher lösen relative Installer-Adressen nicht auf.
    let feed = Feed {
        version: release.version.clone(),
        notes: release.notes.clone(),
        pub_date: release.pub_date.clone(),
        url: format!(
            "{}{}",
            public_base(&state, &headers),
            Store::installer_path(release)
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

// ---------- Download-Seite ----------

const DOWNLOAD_HTML: &str = include_str!("public/download.html");

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `#rgb` oder `#rrggbb` – alles andere landet nicht im Stylesheet.
fn accent(s: &str) -> &str {
    let s = s.trim();
    let hex = s.strip_prefix('#').unwrap_or("");
    if matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        s
    } else {
        "#ffd43f"
    }
}

/// `2026-10-24` → `24.10.2026`
fn date_de(s: &str) -> String {
    match s.splitn(3, '-').collect::<Vec<_>>()[..] {
        [y, m, d] => format!("{d}.{m}.{y}"),
        _ => s.to_string(),
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0).replace('.', ",")
}

pub async fn home(State(state): State<Shared>) -> Response {
    let store = state.store.lock().await;
    let brand = &store.site.brand;
    let name = match brand.name.trim() {
        "" => "MyLauncher",
        n => n,
    };
    let body = match store.current_release() {
        Some(r) => {
            let notes = if r.notes.trim().is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="notes"><h2>Neu in {}</h2><p>{}</p></div>"#,
                    escape(&r.version),
                    escape(r.notes.trim())
                )
            };
            format!(
                r#"<a class="button" href="/download">Für Windows herunterladen</a>
  <p class="meta">Version {} · {} · {}</p>
  {notes}
  <p class="hint">Installer starten und den Anweisungen folgen. Voraussetzung ist der
  <a href="https://prismlauncher.org/download/windows/" rel="noopener">Prism Launcher</a>.
  Updates installiert der Launcher danach selbst.</p>"#,
                escape(&r.version),
                escape(&date_de(&r.pub_date)),
                megabytes(r.size),
            )
        }
        None => r#"<p class="empty">Gerade steht kein Download bereit.</p>"#.to_string(),
    };
    let html = DOWNLOAD_HTML
        .replace("{{accent}}", accent(&brand.accent))
        .replace("{{tagline}}", &escape(brand.tagline.trim()))
        .replace("{{name}}", &escape(name))
        .replace("{{body}}", &body);
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'none'; style-src 'unsafe-inline'; font-src 'self'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'",
                ),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
        ],
        html,
    )
        .into_response()
}

/// Fester Link für Spieler – zeigt immer auf den aktuell angebotenen Installer.
pub async fn download(State(state): State<Shared>) -> Result<Response, ApiError> {
    let store = state.store.lock().await;
    let release = store.current_release().ok_or_else(|| missing("Download"))?;
    let mut res = Redirect::temporary(&Store::installer_path(release)).into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(res)
}

fn static_asset(content_type: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400")),
        ],
        body,
    )
        .into_response()
}

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/font.ttf", get(|| async { static_asset("font/ttf", admin::FONT) }))
        .route("/logo.png", get(|| async { static_asset("image/png", admin::LOGO) }))
        .route("/config.json", get(config))
        .route("/packs/{id}/manifest.json", get(manifest))
        .route("/packs/{id}/files/{sha1}/{name}", get(pack_file))
        .route("/update.json", get(update_feed))
        .route("/updates/{version}/{file}", get(installer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_maskiert_html() {
        assert_eq!(escape(r#"<a href="x">&'"#), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
    }

    #[test]
    fn accent_nur_hex() {
        assert_eq!(accent("#FFD43F"), "#FFD43F");
        assert_eq!(accent("#abc"), "#abc");
        assert_eq!(accent("red;}body{x"), "#ffd43f");
        assert_eq!(accent(""), "#ffd43f");
    }

    #[test]
    fn datum_und_groesse() {
        assert_eq!(date_de("2026-10-24"), "24.10.2026");
        assert_eq!(megabytes(5_163_144), "5,2 MB");
    }
}
