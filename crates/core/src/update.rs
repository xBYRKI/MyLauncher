//! Selbst-Update des Launchers.
//!
//! Ablauf: `update.json` laden → Version vergleichen → Installer herunterladen →
//! SHA-256 **und** Authenticode-Signatur prüfen → Installer starten, Launcher beenden.
//!
//! Die Prüfkette hängt an zwei Dingen, die beide im Launcher selbst eingebaut sind
//! (siehe [`crate::config::update_config`]): der Feed-URL und dem Thumbprint unseres
//! Code-Signing-Zertifikats. Dadurch reicht ein manipulierter Feed nicht aus – ein
//! untergeschobener Installer müsste zusätzlich mit unserem Zertifikat signiert sein.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::TryStreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::authenticode;
use crate::config::{self, UpdateConfig};

pub use protocol::release::Feed;

/// Ergebnis einer Prüfung – wird direkt an die Oberfläche gegeben.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Ist überhaupt ein Feed hinterlegt?
    pub configured: bool,
    pub current: String,
    pub available: bool,
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub size: u64,
    pub checked_at: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: &'static str,
    pub bytes: u64,
    pub total_bytes: u64,
}

// ---------- Versionsvergleich ----------

/// `0.10.0` ist neuer als `0.9.0`, und `0.3.0` neuer als `0.3.0-beta.1`.
pub fn compare(a: &str, b: &str) -> Ordering {
    protocol::release::compare_versions(a, b)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    compare(candidate, current) == Ordering::Greater
}

// ---------- Feed ----------

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(Into::into)
}

pub async fn fetch_feed(url: &str) -> Result<Feed> {
    let text = crate::modpack::fetch_source(url)
        .await
        .with_context(|| format!("Update-Feed nicht erreichbar: {url}"))?;
    let feed: Feed = serde_json::from_str(&text).context("update.json ist kein gültiges JSON")?;
    if feed.version.trim().is_empty() || feed.url.trim().is_empty() {
        bail!("update.json braucht \"version\" und \"url\"");
    }
    Ok(feed)
}

/// Prüft auf eine neue Version. Schlägt nie fehl – Fehler stehen in `error`,
/// damit ein nicht erreichbarer Server den Launcher nicht ausbremst.
pub async fn check(current: &str) -> UpdateStatus {
    let cfg = config::update_config();
    let mut status = UpdateStatus {
        configured: !cfg.feed.trim().is_empty(),
        current: current.to_string(),
        checked_at: crate::now_millis(),
        ..Default::default()
    };
    if !status.configured {
        return status;
    }
    match fetch_feed(&cfg.feed).await {
        Ok(feed) => {
            status.available = is_newer(&feed.version, current);
            status.version = feed.version;
            status.notes = feed.notes;
            status.pub_date = feed.pub_date;
            status.size = feed.size.unwrap_or(0);
        }
        Err(e) => status.error = Some(format!("{e:#}")),
    }
    status
}

// ---------- Herunterladen ----------

fn installer_name(url: &str) -> String {
    let name = url
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if safe.len() > 4 && safe.to_ascii_lowercase().ends_with(".exe") {
        safe
    } else {
        "MyLauncher-setup.exe".into()
    }
}

/// Lädt den Installer nach `%APPDATA%\MyLauncher\updates` und prüft Prüfsumme und Signatur.
/// Gibt den Pfad zur geprüften Datei zurück.
pub async fn download(cfg: &UpdateConfig, feed: &Feed, on_progress: impl Fn(Progress)) -> Result<PathBuf> {
    let dir = config::app_dir().join("updates");
    tokio::fs::create_dir_all(&dir).await?;
    // Reste früherer Versuche wegräumen, damit der Ordner nicht wächst.
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }

    let dest = dir.join(installer_name(&feed.url));
    let part = dest.with_extension("part");
    let mut hasher = Sha256::new();
    let mut total = feed.size.unwrap_or(0);
    let mut bytes = 0u64;

    // Relative Installer-URLs gelten relativ zum Feed (so liefert sie der eigene Server).
    let url = crate::modpack::to_url(cfg.feed.trim())?.join(feed.url.trim())?;
    let result: Result<()> = async {
        let res = http()?
            .get(url)
            .timeout(Duration::from_secs(600))
            .send()
            .await?;
        if !res.status().is_success() {
            bail!("Download fehlgeschlagen (HTTP {})", res.status());
        }
        if total == 0 {
            total = res.content_length().unwrap_or(0);
        }
        let mut out = tokio::fs::File::create(&part).await?;
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.try_next().await? {
            hasher.update(&chunk);
            bytes += chunk.len() as u64;
            out.write_all(&chunk).await?;
            on_progress(Progress {
                phase: "download",
                bytes,
                total_bytes: total,
            });
        }
        out.flush().await?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    on_progress(Progress {
        phase: "verify",
        bytes,
        total_bytes: total,
    });
    let actual = hex::encode(hasher.finalize());
    if !feed.sha256.trim().is_empty() && !actual.eq_ignore_ascii_case(feed.sha256.trim()) {
        let _ = tokio::fs::remove_file(&part).await;
        bail!("Prüfsumme des Installers stimmt nicht – Download abgebrochen.");
    }
    tokio::fs::rename(&part, &dest).await?;

    // Entscheidend: Nur ein mit unserem Zertifikat signierter Installer darf laufen.
    // Ohne gültige Signatur wird die Datei sofort gelöscht.
    match authenticode::verify(&dest, &cfg.signer_thumbprint) {
        Ok(signer) => {
            let _ = signer;
            Ok(dest)
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(e)
        }
    }
}

/// Startet den geprüften Installer. Danach muss der Launcher sich beenden,
/// sonst kann der Installer die laufende Exe nicht ersetzen.
pub fn start_installer(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("Installer nicht gefunden: {}", path.display()));
    }
    std::process::Command::new(path)
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .spawn()
        .with_context(|| format!("Installer konnte nicht gestartet werden: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versionen_werden_numerisch_verglichen() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // Führendes v und fehlende Stellen
        assert!(is_newer("v0.3", "0.2.9"));
        assert!(!is_newer("0.2", "0.2.0"));
    }

    #[test]
    fn vorabversionen_sind_aelter() {
        assert!(is_newer("0.3.0", "0.3.0-beta.1"));
        assert!(!is_newer("0.3.0-beta.1", "0.3.0"));
        assert!(is_newer("0.3.0-beta.2", "0.3.0-beta.1"));
    }

    #[test]
    fn dateiname_wird_entschaerft() {
        assert_eq!(
            installer_name("https://example.net/r/MyLauncher_0.3.0_x64-setup.exe"),
            "MyLauncher_0.3.0_x64-setup.exe"
        );
        // Pfadtrenner und Query fallen weg – der Name landet immer in unserem Update-Ordner.
        assert_eq!(
            installer_name("https://example.net/a/..\\..\\evil.exe?x=1"),
            "evil.exe"
        );
        assert_eq!(installer_name("https://example.net/pfad/"), "MyLauncher-setup.exe");
        assert_eq!(installer_name("https://example.net/x.zip"), "MyLauncher-setup.exe");
    }
}
