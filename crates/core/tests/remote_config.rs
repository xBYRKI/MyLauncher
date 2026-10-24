//! End-to-End-Test gegen einen laufenden `mylauncher-server` mit mindestens einem
//! veröffentlichten, im Launcher sichtbaren Modpack:
//! `LAUNCHER_TEST_URL=http://127.0.0.1:8787 cargo test -p launcher-core --test remote_config -- --ignored`

use std::path::PathBuf;

use launcher_core::{config, modpack, prism::Prism};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mylauncher-remote-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
#[ignore = "braucht einen laufenden mylauncher-server (LAUNCHER_TEST_URL)"]
async fn packs_kommen_vom_server_und_lassen_sich_installieren() {
    let base = std::env::var("LAUNCHER_TEST_URL").unwrap();
    let home = temp("home");
    // Einziger Test in dieser Datei – die Umgebung gehört ihm allein.
    std::env::set_var("MYLAUNCHER_HOME", &home);
    std::fs::write(
        home.join("launcher.config.json"),
        serde_json::json!({ "remoteConfig": format!("{base}/launcher/config.json") }).to_string(),
    )
    .unwrap();

    assert!(config::refresh_remote_config().await.unwrap(), "erster Abruf ändert etwas");
    assert!(!config::refresh_remote_config().await.unwrap(), "zweiter Abruf nicht mehr");

    let cfg = config::load_launcher_config();
    let source = &cfg.packs.first().expect("Server liefert ein Pack").manifest;
    assert!(source.starts_with(&base), "relative Manifest-URL aufgelöst: {source}");
    // Feed und Thumbprint bleiben die eingebauten.
    assert_eq!(config::update_config().feed, {
        let bundled: serde_json::Value =
            serde_json::from_str(include_str!("../../../launcher.config.json")).unwrap();
        bundled["update"]["feed"].as_str().unwrap_or_default().to_string()
    });

    let instances = temp("instances");
    let prism = Prism {
        found: true,
        exe_found: false,
        data_found: true,
        cmd: String::new(),
        base_args: vec![],
        exe: String::new(),
        data_dir: instances.clone(),
        default_data_dir: instances.clone(),
        portable: false,
        instances_dir: instances.clone(),
        icons_dir: instances.join("icons"),
    };
    let result = modpack::sync_pack(&prism, source, "Test", |_| {}).await.unwrap();
    let plan = modpack::plan_pack(&prism, source, |_| {}).await.unwrap();
    assert!(plan.changes.is_empty(), "nach dem Sync ist alles aktuell: {:?}", plan.changes);
    assert!(instances.join(&result.instance_id).join("mmc-pack.json").is_file());
    println!("{} {} – {} Dateien", result.instance_id, result.version, result.downloaded);
}
