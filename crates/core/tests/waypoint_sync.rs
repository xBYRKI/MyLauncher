//! End-to-End-Test gegen einen laufenden Sync-Server:
//! `WAYPOINT_TEST_URL=http://127.0.0.1:8787 WAYPOINT_TEST_TOKEN=… cargo test -p launcher-core -- --ignored`

use std::path::{Path, PathBuf};

use launcher_core::waypoints::{sync_world, SyncOptions};
use launcher_core::xaero::WaypointFile;

const WORLD: &str = "Multiplayer_test.example.net";

fn opts(player: &str) -> SyncOptions {
    SyncOptions {
        server_url: std::env::var("WAYPOINT_TEST_URL").unwrap(),
        token: std::env::var("WAYPOINT_TEST_TOKEN").unwrap(),
        player: player.into(),
        shared_set: "Team".into(),
        share_all: false,
    }
}

fn game_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mylauncher-wp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write(game: &Path, dim: &str, lines: &[&str]) {
    let dir = game.join("xaero/minimap").join(WORLD).join(dim);
    std::fs::create_dir_all(&dir).unwrap();
    let mut text = "#\n#waypoint:name:initials:x:y:z:color:disabled:type:set:rotate_on_tp:tp_yaw:visibility_type:destination\n#\n".to_string();
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(dir.join("mw$default_1.txt"), text).unwrap();
}

fn names(game: &Path, dim: &str) -> Vec<String> {
    let path = game
        .join("xaero/minimap")
        .join(WORLD)
        .join(dim)
        .join("mw$default_1.txt");
    let mut v: Vec<String> = WaypointFile::load(&path)
        .map(|f| f.waypoints().map(|w| w.name.clone()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[tokio::test]
#[ignore]
async fn two_players_share_waypoints() {
    let a = game_dir("a");
    let b = game_dir("b");
    write(
        &a,
        "dim%0",
        &[
            "waypoint:Privat A:P:1:64:1:2:false:0:gui.xaero_default:false:0:0:false",
            "waypoint:Basis §§ Nord:B:100:70:-200:5:false:0:Team:false:0:0:false",
            "waypoint:Tod:X:5:60:5:0:false:1:Team:false:0:0:false",
        ],
    );
    write(
        &b,
        "dim%0",
        &["waypoint:Privat B:Q:9:64:9:3:false:0:gui.xaero_default:false:0:0:false"],
    );

    let r = sync_world(&a, WORLD, &opts("Alice")).await.unwrap();
    assert_eq!(
        (r.pushed_added, r.total),
        (1, 1),
        "Todespunkt darf nicht geteilt werden"
    );

    let r = sync_world(&b, WORLD, &opts("Bob")).await.unwrap();
    assert_eq!(r.pulled_added, 1);
    assert_eq!(names(&b, "dim%0"), ["Basis : Nord", "Privat B"]);

    // Bob löscht "Basis : Nord" und setzt einen Netherwegpunkt (neue Datei)
    write(
        &b,
        "dim%0",
        &["waypoint:Privat B:Q:9:64:9:3:false:0:gui.xaero_default:false:0:0:false"],
    );
    write(
        &b,
        "dim%-1",
        &["waypoint:Portal:N:12:40:-25:10:false:0:Team:false:0:0:false"],
    );
    let r = sync_world(&b, WORLD, &opts("Bob")).await.unwrap();
    assert_eq!((r.pushed_added, r.pushed_deleted, r.total), (1, 1, 1));

    // Alice übernimmt beides; Privates und Todespunkt bleiben
    sync_world(&a, WORLD, &opts("Alice")).await.unwrap();
    assert_eq!(names(&a, "dim%0"), ["Privat A", "Tod"]);
    assert_eq!(names(&a, "dim%-1"), ["Portal"]);

    // Ein weiterer Sync ändert nichts mehr
    let before = std::fs::read_to_string(
        a.join("xaero/minimap")
            .join(WORLD)
            .join("dim%0/mw$default_1.txt"),
    )
    .unwrap();
    let r = sync_world(&a, WORLD, &opts("Alice")).await.unwrap();
    assert_eq!(
        (
            r.pushed_added,
            r.pushed_deleted,
            r.pulled_added,
            r.pulled_removed
        ),
        (0, 0, 0, 0)
    );
    let after = std::fs::read_to_string(
        a.join("xaero/minimap")
            .join(WORLD)
            .join("dim%0/mw$default_1.txt"),
    )
    .unwrap();
    assert_eq!(before, after);

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
