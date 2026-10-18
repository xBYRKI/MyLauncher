//! Gemeinsame Typen für den Wegpunkt-Sync zwischen Launcher und Sync-Server.

use serde::{Deserialize, Serialize};

/// Inhalt eines geteilten Xaero-Wegpunkts. Spielerspezifisches (z. B. "ausgeblendet")
/// wird bewusst nicht geteilt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaypointData {
    /// Xaero-Dimensionsordner, z. B. `dim%0`, `dim%-1`.
    pub dim: String,
    /// Xaero-Wegpunktdatei ohne `.txt`, z. B. `mw$default_1`.
    pub world: String,
    pub name: String,
    pub initials: String,
    pub x: i64,
    /// Xaero erlaubt `~` für "Höhe unbekannt", daher als Text.
    pub y: String,
    pub z: i64,
    pub color: u8,
    /// Restliche Xaero-Felder (rotate_on_tp, tp_yaw, visibility_type, destination, …).
    #[serde(default)]
    pub extra: Vec<String>,
}

impl WaypointData {
    /// Identität für den Abgleich: gleicher Inhalt = gleicher Wegpunkt.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            self.dim, self.world, self.name, self.initials, self.x, self.y, self.z, self.color
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWaypoint {
    pub id: String,
    #[serde(flatten)]
    pub data: WaypointData,
    pub author: String,
    pub created_at: u64,
}

/// Lokale Änderungen seit dem letzten Sync.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub player: String,
    pub added: Vec<WaypointData>,
    pub deleted: Vec<String>,
}

/// Aktueller Stand eines Raums (= einer Xaero-Welt, z. B. `Multiplayer_play.example.net`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomState {
    pub rev: u64,
    pub waypoints: Vec<SharedWaypoint>,
}

/// Erlaubte Zeichen in Raumnamen (Xaero-Ordnernamen, ohne Pfadtrenner).
pub fn valid_room(room: &str) -> bool {
    !room.is_empty()
        && room.len() <= 200
        && room != "."
        && room != ".."
        && !room.chars().any(|c| {
            matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
}
