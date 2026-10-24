//! Wegpunkt-Sync: geteilte Xaero-Wegpunkte pro Team und Welt als JSON-Dateien
//! (`<data>/<team>/<welt>.json`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use protocol::{RoomState, SharedWaypoint, SyncRequest, WaypointData};
use serde_json::{json, Value};

use crate::{now_millis, ApiError, AppState, Shared};

const MAX_WAYPOINTS_PER_ROOM: usize = 5000;
const MAX_TEXT: usize = 100;

fn team_for(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    state
        .tokens
        .get(token)
        .cloned()
        .ok_or((StatusCode::UNAUTHORIZED, "Ungültiger Team-Schlüssel".into()))
}

fn room_file(state: &AppState, team: &str, room: &str) -> PathBuf {
    state.data_dir.join(team).join(format!("{room}.json"))
}

async fn load_room<'a>(
    state: &AppState,
    rooms: &'a mut HashMap<(String, String), RoomState>,
    team: &str,
    room: &str,
) -> &'a mut RoomState {
    let key = (team.to_string(), room.to_string());
    if !rooms.contains_key(&key) {
        let loaded = tokio::fs::read_to_string(room_file(state, team, room))
            .await
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        rooms.insert(key.clone(), loaded);
    }
    rooms.get_mut(&key).expect("gerade eingefügt")
}

async fn save_room(
    state: &AppState,
    team: &str,
    room: &str,
    data: &RoomState,
) -> std::io::Result<()> {
    let path = room_file(state, team, room);
    tokio::fs::create_dir_all(path.parent().expect("hat Elternordner")).await?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(data)?).await?;
    tokio::fs::rename(&tmp, &path).await
}

fn validate(w: &WaypointData) -> bool {
    let short = |s: &str| s.chars().count() <= MAX_TEXT && !s.contains('\n');
    w.dim.starts_with("dim%")
        && protocol::valid_room(&w.dim)
        && protocol::valid_room(&w.world)
        && short(&w.name)
        && short(&w.initials)
        && short(&w.y)
        && !w.name.is_empty()
        && w.extra.len() <= 16
        && w.extra.iter().all(|e| short(e) && !e.contains(':'))
}

async fn health(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let team = team_for(&state, &headers)?;
    Ok(Json(json!({ "ok": true, "team": team })))
}

async fn get_room(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(room): Path<String>,
) -> Result<Json<RoomState>, ApiError> {
    let team = team_for(&state, &headers)?;
    if !protocol::valid_room(&room) {
        return Err((StatusCode::BAD_REQUEST, "Ungültiger Raum".into()));
    }
    let mut rooms = state.rooms.lock().await;
    Ok(Json(
        load_room(&state, &mut rooms, &team, &room).await.clone(),
    ))
}

async fn sync_room(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(room): Path<String>,
    Json(req): Json<SyncRequest>,
) -> Result<Json<RoomState>, ApiError> {
    let team = team_for(&state, &headers)?;
    if !protocol::valid_room(&room) {
        return Err((StatusCode::BAD_REQUEST, "Ungültiger Raum".into()));
    }
    let player: String = req.player.chars().take(32).collect();

    let mut rooms = state.rooms.lock().await;
    let data = load_room(&state, &mut rooms, &team, &room).await;
    let before = data.waypoints.len();

    let deleted: HashSet<&String> = req.deleted.iter().collect();
    data.waypoints.retain(|w| !deleted.contains(&w.id));
    let mut changed = data.waypoints.len() != before;

    let mut keys: HashSet<String> = data.waypoints.iter().map(|w| w.data.key()).collect();
    for w in req.added.into_iter().filter(validate) {
        if data.waypoints.len() >= MAX_WAYPOINTS_PER_ROOM {
            break;
        }
        if keys.insert(w.key()) {
            data.waypoints.push(SharedWaypoint {
                id: uuid::Uuid::new_v4().to_string(),
                data: w,
                author: player.clone(),
                created_at: now_millis(),
            });
            changed = true;
        }
    }

    if changed {
        data.rev += 1;
        save_room(&state, &team, &room, data).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Speichern fehlgeschlagen: {e}"),
            )
        })?;
        println!(
            "[{team}/{room}] rev {} von {player}: {} Wegpunkte",
            data.rev,
            data.waypoints.len()
        );
    }
    Ok(Json(data.clone()))
}

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/health", get(health))
        .route("/rooms/{room}", get(get_room))
        .route("/rooms/{room}/sync", post(sync_room))
}
