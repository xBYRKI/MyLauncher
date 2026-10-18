// Release-Builds ohne Konsolenfenster (auch wenn Prism uns als Hook startet).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod theme;

use std::io::Write;
use std::path::PathBuf;

use launcher_core::{config, waypoints};

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == waypoints::HOOK_FLAG) {
        run_hook(&args[pos + 1..]);
        return Ok(());
    }
    app::run()
}

/// Von Prism aufgerufen: `mylauncher --xaero-sync <pre|post> <INST_MC_DIR>`.
/// Endet immer mit Exit-Code 0 – ein Sync-Fehler darf den Spielstart nicht blockieren.
fn run_hook(args: &[String]) {
    let phase = args.first().map(String::as_str).unwrap_or("pre");
    let game_dir = args.get(1).map(PathBuf::from).unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Tokio-Runtime");
    let lines = runtime.block_on(waypoints::run_hook(phase, &game_dir));

    let log_path = config::app_dir().join("waypoint-sync.log");
    let _ = std::fs::create_dir_all(config::app_dir());
    // Log klein halten
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 512 * 1024) {
        let _ = std::fs::remove_file(&log_path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let stamp = launcher_core::now_millis() / 1000;
        for line in &lines {
            println!("{line}");
            let _ = writeln!(f, "{stamp} {line}");
        }
    }
}
