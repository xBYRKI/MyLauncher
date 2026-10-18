//! Brücke zu Prism Launcher: Installation finden, Instanzen/Konten lesen, über die CLI starten.
//! Prism bleibt die Engine (Login, Java, Loader, Assets). Wir lesen nur dessen Daten
//! und rufen `prismlauncher --launch/--show` auf.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Result};
use base64::Engine;
use serde::Serialize;
use serde_json::Value;

use crate::config::Settings;
use crate::ini::Ini;

const FLATPAK_ID: &str = "org.prismlauncher.PrismLauncher";
const ICON_EXTS: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
];
pub const LOADERS: &[(&str, &str)] = &[
    ("net.fabricmc.fabric-loader", "Fabric"),
    ("org.quiltmc.quilt-loader", "Quilt"),
    ("net.minecraftforge", "Forge"),
    ("net.neoforged", "NeoForge"),
    ("com.mumfrey.liteloader", "LiteLoader"),
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prism {
    pub found: bool,
    pub exe_found: bool,
    pub data_found: bool,
    #[serde(skip)]
    pub cmd: String,
    #[serde(skip)]
    pub base_args: Vec<String>,
    pub exe: String,
    pub data_dir: PathBuf,
    pub default_data_dir: PathBuf,
    pub portable: bool,
    pub instances_dir: PathBuf,
    pub icons_dir: PathBuf,
}

// ---------- Erkennung ----------

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn default_data_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        vec![
            dirs::data_dir()
                .unwrap_or_else(|| home().join(".local/share"))
                .join("PrismLauncher"),
            home()
                .join(".var/app")
                .join(FLATPAK_ID)
                .join("data/PrismLauncher"),
        ]
    } else {
        // Windows: %APPDATA%, macOS: ~/Library/Application Support
        vec![dirs::data_dir().unwrap_or_default().join("PrismLauncher")]
    }
}

fn exe_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        let local = dirs::data_local_dir().unwrap_or_default();
        let program_files = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| "C:\\Program Files".into());
        vec![
            local.join("Programs/PrismLauncher/prismlauncher.exe"),
            program_files.join("PrismLauncher/prismlauncher.exe"),
            home().join("scoop/apps/prismlauncher/current/prismlauncher.exe"),
        ]
    } else if cfg!(target_os = "macos") {
        vec!["/Applications/Prism Launcher.app/Contents/MacOS/prismlauncher".into()]
    } else {
        vec![
            "/usr/bin/prismlauncher".into(),
            "/usr/local/bin/prismlauncher".into(),
            home().join(".local/bin/prismlauncher"),
        ]
    }
}

pub fn resolve(settings: &Settings) -> Prism {
    let configured = PathBuf::from(&settings.prism_exe);
    let mut exe = if !settings.prism_exe.is_empty() && configured.exists() {
        Some(configured)
    } else {
        exe_candidates().into_iter().find(|p| p.exists())
    };
    let mut cmd = exe
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut base_args = vec![];
    let mut exe_label = cmd.clone();
    if exe.is_none()
        && cfg!(target_os = "linux")
        && home().join(".var/app").join(FLATPAK_ID).exists()
    {
        cmd = "flatpak".into();
        base_args = vec!["run".into(), FLATPAK_ID.into()];
        exe_label = format!("flatpak {FLATPAK_ID}");
        exe = None;
    }

    // Portable-Installationen speichern alles neben der Exe.
    let portable = exe
        .as_ref()
        .and_then(|e| e.parent())
        .is_some_and(|d| d.join("portable.txt").exists());
    let default_data_dir = if portable {
        exe.as_ref()
            .and_then(|e| e.parent())
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        let dirs = default_data_dirs();
        dirs.iter()
            .find(|d| d.exists())
            .cloned()
            .unwrap_or_else(|| dirs[0].clone())
    };
    let data_dir = if settings.prism_data_dir.is_empty() {
        default_data_dir.clone()
    } else {
        PathBuf::from(&settings.prism_data_dir)
    };

    let cfg = Ini::load(&data_dir.join("prismlauncher.cfg"));
    let instances_dir = data_dir.join(
        cfg.general("InstanceDir")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "instances".into()),
    );
    let icons_dir = data_dir.join(
        cfg.general("IconsDir")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "icons".into()),
    );

    Prism {
        found: !cmd.is_empty() && data_dir.exists(),
        exe_found: !cmd.is_empty(),
        data_found: data_dir.exists(),
        cmd,
        base_args,
        exe: exe_label,
        data_dir,
        default_data_dir,
        portable,
        instances_dir,
        icons_dir,
    }
}

// ---------- Instanzen ----------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loader {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedBy {
    pub pack_id: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: Option<Loader>,
    pub group: Option<String>,
    pub icon: Option<String>,
    pub last_launch: u64,
    pub total_played: u64,
    pub managed_by: Option<ManagedBy>,
    pub waypoint_hooks: bool,
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Minecraft-Version und Loader aus `mmc-pack.json`.
pub fn describe_pack(pack: Option<&Value>) -> (String, Option<Loader>) {
    let empty = vec![];
    let components = pack
        .and_then(|p| p["components"].as_array())
        .unwrap_or(&empty);
    let version_of = |c: &Value| {
        c["version"]
            .as_str()
            .or_else(|| c["cachedVersion"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let mc = components
        .iter()
        .find(|c| c["uid"] == "net.minecraft")
        .map(version_of)
        .unwrap_or_else(|| "?".into());
    let loader = components.iter().find_map(|c| {
        let uid = c["uid"].as_str()?;
        let (_, name) = LOADERS.iter().find(|(u, _)| *u == uid)?;
        Some(Loader {
            name: (*name).into(),
            version: version_of(c),
        })
    });
    (mc, loader)
}

fn icon_data_url(icons_dir: &Path, key: &str) -> Option<String> {
    if key.is_empty() || key == "default" {
        return None;
    }
    ICON_EXTS.iter().find_map(|(ext, mime)| {
        let bytes = std::fs::read(icons_dir.join(format!("{key}.{ext}"))).ok()?;
        Some(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    })
}

fn groups_by_instance(instances_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(Value::Object(groups)) =
        read_json(&instances_dir.join("instgroups.json")).map(|v| v["groups"].clone())
    {
        for (group, info) in groups {
            for id in info["instances"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                map.insert(id.to_string(), group.clone());
            }
        }
    }
    map
}

/// Spielordner einer Instanz (`minecraft`, bei alten Instanzen `.minecraft`).
pub fn game_dir(instance_dir: &Path) -> PathBuf {
    let dotted = instance_dir.join(".minecraft");
    if dotted.exists() && !instance_dir.join("minecraft").exists() {
        dotted
    } else {
        instance_dir.join("minecraft")
    }
}

pub fn instance_dir(prism: &Prism, id: &str) -> Result<PathBuf> {
    let name = Path::new(id)
        .file_name()
        .ok_or_else(|| anyhow!("Ungültige Instanz"))?;
    let dir = prism.instances_dir.join(name);
    if !dir.join("instance.cfg").exists() {
        return Err(anyhow!("Instanz \"{id}\" nicht gefunden"));
    }
    Ok(dir)
}

pub fn list_instances(prism: &Prism, hidden: &[String]) -> Vec<Instance> {
    let Ok(entries) = std::fs::read_dir(&prism.instances_dir) else {
        return vec![];
    };
    let groups = groups_by_instance(&prism.instances_dir);
    let mut out: Vec<Instance> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            // Prism nutzt _LAUNCHER_TEMP, _MMC_TEMP usw. intern.
            if id.starts_with('_')
                || id.starts_with('.')
                || hidden.iter().any(|h| h.eq_ignore_ascii_case(&id))
            {
                return None;
            }
            let dir = e.path();
            let cfg_path = dir.join("instance.cfg");
            if !cfg_path.exists() {
                return None;
            }
            let cfg = Ini::load(&cfg_path);
            let (mc_version, loader) =
                describe_pack(read_json(&dir.join("mmc-pack.json")).as_ref());
            let managed_by = read_json(&dir.join(crate::modpack::STATE_FILE)).map(|m| ManagedBy {
                pack_id: m["packId"].as_str().unwrap_or_default().into(),
                version: m["version"].as_str().unwrap_or_default().into(),
            });
            let num = |k: &str| {
                cfg.general(k)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            Some(Instance {
                name: cfg
                    .general("name")
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| id.clone()),
                group: groups.get(&id).cloned(),
                icon: icon_data_url(
                    &prism.icons_dir,
                    &cfg.general("iconKey").unwrap_or_default(),
                ),
                last_launch: num("lastLaunchTime"),
                total_played: num("totalTimePlayed"),
                waypoint_hooks: crate::waypoints::hooks_installed(&cfg),
                id,
                mc_version,
                loader,
                managed_by,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_launch
            .cmp(&a.last_launch)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

// ---------- Konten ----------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub uuid: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub active: bool,
}

/// Nur Name/UUID verlassen diese Funktion – Tokens bleiben in Prism.
pub fn list_accounts(prism: &Prism) -> Vec<Account> {
    let data = read_json(&prism.data_dir.join("accounts.json")).unwrap_or(Value::Null);
    data["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(Account {
                name: a["profile"]["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())?
                    .into(),
                uuid: a["profile"]["id"].as_str().unwrap_or_default().into(),
                kind: a["type"].as_str().unwrap_or("MSA").into(),
                active: a["active"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Gewähltes Konto oder Prism-Standard – als Autor für geteilte Wegpunkte.
pub fn player_name(prism: &Prism, settings: &Settings) -> String {
    let accounts = list_accounts(prism);
    accounts
        .iter()
        .find(|a| a.name == settings.account)
        .or_else(|| accounts.iter().find(|a| a.active))
        .or(accounts.first())
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Unbekannt".into())
}

// ---------- Starten ----------

fn run(prism: &Prism, extra: &[String]) -> Result<()> {
    if prism.cmd.is_empty() {
        return Err(anyhow!(
            "Prism Launcher wurde nicht gefunden. Pfad in den Einstellungen setzen."
        ));
    }
    let mut args = prism.base_args.clone();
    let custom_dir = !prism.portable && prism.data_dir != prism.default_data_dir;
    if custom_dir {
        args.push("--dir".into());
        args.push(prism.data_dir.to_string_lossy().into_owned());
    }
    args.extend_from_slice(extra);

    let mut command = Command::new(&prism.cmd);
    command.args(&args);
    if let Some(dir) = Path::new(&prism.cmd).parent().filter(|d| d.is_dir()) {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command
        .spawn()
        .map_err(|e| anyhow!("Prism konnte nicht gestartet werden: {e}"))?;
    Ok(())
}

/// Startet eine Instanz. Läuft Prism schon, leitet es den Befehl an das offene Fenster weiter.
pub fn launch(prism: &Prism, instance_id: &str, account: &str, server: &str) -> Result<()> {
    let mut args = vec!["--launch".to_string(), instance_id.to_string()];
    if !account.is_empty() {
        args.extend(["--profile".into(), account.into()]);
    }
    if !server.is_empty() {
        args.extend(["--server".into(), server.into()]);
    }
    run(prism, &args)
}

pub fn show(prism: &Prism, instance_id: Option<&str>) -> Result<()> {
    match instance_id {
        Some(id) => run(prism, &["--show".into(), id.into()]),
        None => run(prism, &[]),
    }
}

// ---------- Prozesse ----------

fn processes() -> sysinfo::System {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
    );
    sys
}

/// Läuft Prism gerade? (Dann dürfen wir `instance.cfg` nicht anfassen – Prism würde es überschreiben.)
pub fn is_prism_running() -> bool {
    processes().processes().values().any(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        name == "prismlauncher.exe" || name == "prismlauncher"
    })
}

/// Läuft Minecraft aus diesem Instanzordner? (Java-Prozess mit Instanzpfad in der Kommandozeile)
pub fn is_instance_running(instance_dir: &Path) -> bool {
    let needle = instance_dir
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    processes().processes().values().any(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        name.starts_with("java")
            && p.cmd().iter().any(|a| {
                a.to_string_lossy()
                    .replace('\\', "/")
                    .to_lowercase()
                    .contains(&needle)
            })
    })
}
