//! Wegpunkt-Sync-Server für MyLauncher.
//!
//! Speichert geteilte Xaero-Wegpunkte pro Team und Welt als JSON-Dateien.
//!
//! ```text
//! waypoint-server --bind 0.0.0.0:8787 --data ./data --token freunde:GEHEIM [--token team2:GEHEIM2]
//! ```
//! Alternativ per Umgebung: `WAYPOINT_BIND`, `WAYPOINT_DATA`, `WAYPOINT_TOKENS="freunde:GEHEIM,team2:GEHEIM2"`.
//! Für Internet-Betrieb einen Reverse-Proxy mit HTTPS davorschalten (Caddy, nginx).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use protocol::{RoomState, SharedWaypoint, SyncRequest, WaypointData};
use serde_json::{json, Value};
use tokio::sync::Mutex;

const MAX_WAYPOINTS_PER_ROOM: usize = 5000;
const MAX_TEXT: usize = 100;

struct AppState {
    data_dir: PathBuf,
    /// Token -> Teamname
    tokens: HashMap<String, String>,
    rooms: Mutex<HashMap<(String, String), RoomState>>,
}

type Shared = Arc<AppState>;
type ApiError = (StatusCode, String);

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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

struct Args {
    bind: SocketAddr,
    data: PathBuf,
    tokens: HashMap<String, String>,
}

fn parse_args() -> Result<Args> {
    let mut bind = std::env::var("WAYPOINT_BIND").unwrap_or_else(|_| "0.0.0.0:8787".into());
    let mut data = std::env::var("WAYPOINT_DATA").unwrap_or_else(|_| "data".into());
    let mut raw_tokens: Vec<String> = std::env::var("WAYPOINT_TOKENS")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next()
                .with_context(|| format!("{a} braucht einen Wert"))
        };
        match a.as_str() {
            "--bind" => bind = value()?,
            "--data" => data = value()?,
            "--token" => raw_tokens.push(value()?),
            "-h" | "--help" => {
                println!("waypoint-server --bind 0.0.0.0:8787 --data ./data --token team:GEHEIM");
                std::process::exit(0);
            }
            other => bail!("Unbekanntes Argument: {other}"),
        }
    }

    let mut tokens = HashMap::new();
    for raw in raw_tokens {
        let (team, secret) = raw.split_once(':').unwrap_or(("default", raw.as_str()));
        if secret.len() < 8 {
            bail!("Token für Team \"{team}\" ist zu kurz (mind. 8 Zeichen)");
        }
        if !protocol::valid_room(team) {
            bail!("Ungültiger Teamname: {team}");
        }
        tokens.insert(secret.to_string(), team.to_string());
    }
    if tokens.is_empty() {
        bail!("Mindestens ein --token team:GEHEIM angeben (oder WAYPOINT_TOKENS setzen)");
    }
    Ok(Args {
        bind: bind.parse().context("Ungültige --bind-Adresse")?,
        data: PathBuf::from(data),
        tokens,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    std::fs::create_dir_all(&args.data)?;
    let teams: HashSet<&String> = args.tokens.values().collect();
    println!(
        "Wegpunkt-Server auf http://{} – Daten in {} – Teams: {:?}",
        args.bind,
        args.data.display(),
        teams
    );

    let state = Arc::new(AppState {
        data_dir: args.data,
        tokens: args.tokens,
        rooms: Mutex::new(HashMap::new()),
    });
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/rooms/{room}", get(get_room))
        .route("/api/rooms/{room}/sync", post(sync_room))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
