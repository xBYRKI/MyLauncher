# MyLauncher

Eigener Minecraft-Launcher in **Rust** (Tauri 2). **Prism Launcher läuft darunter als Engine:**
Prism übernimmt Microsoft-Login, Java, Loader-Installation und den Spielstart. MyLauncher liest Prisms Daten
direkt, steuert Prism über dessen Kommandozeile und klinkt sich für den Xaero-Wegpunkt-Sync als Prism-Hook ein.

```
MyLauncher (Tauri, Rust)
  ├─ liest     instances/*/instance.cfg, mmc-pack.json, accounts.json (nur Name/UUID)
  ├─ startet   prismlauncher --launch <Instanz> --profile <Konto> [--server <Adresse>]
  ├─ Modpacks  legt Prism-Instanzen an und hält Dateien per SHA1 aktuell
  ├─ Update    prüft update.json, lädt den Installer und akzeptiert nur unsere Signatur
  └─ Hooks     PreLaunchCommand / PostExitCommand = mylauncher --xaero-sync pre|post "$INST_MC_DIR"
                  └─ waypoint-server (eigener Rust-Server, teilt Wegpunkte im Team)
```

## Aufbau

| Ordner | Inhalt |
|---|---|
| `app/` | Tauri-App (Befehle, Hook-Modus `--xaero-sync`) |
| `ui/` | Oberfläche (HTML/CSS/JS, kein Build-Schritt) |
| `crates/core/` | Prism-Brücke, Modpack-Sync, Server-Ping, Xaero-Parser, Wegpunkt-Sync; `packtool` |
| `crates/server/` | `waypoint-server` – Sync-Server für Wegpunkte |
| `crates/protocol/` | Gemeinsame Typen von Launcher und Server |

## Bauen & Starten

```bash
cargo run -p mylauncher                  # Launcher (Entwicklung)
cargo build --release -p mylauncher      # target/release/mylauncher.exe
cargo install tauri-cli --version "^2"   # einmalig, für den Installer:
cd app && cargo tauri build              # NSIS-Installer
cargo test -p launcher-core              # Tests
```

### Signierte Veröffentlichung

```powershell
.\scripts\build-signed.ps1               # Installer + Tools, signiert, nach dist\
.\scripts\build-signed.ps1 -SkipTools    # nur den Launcher-Installer
```

Baut alles als Release und signiert es mit dem Certum-Code-Signing-Zertifikat (SHA-256, Zeitstempel
von `time.certum.pl`). Dafür muss **SimplySign Desktop** laufen und angemeldet sein – erst dann liegt
das Zertifikat im Windows-Zertifikatsspeicher und der private Schlüssel ist erreichbar. Das Skript
prüft das vorab mit einer Probe-Signatur und bricht sonst mit einem Hinweis ab. Ergebnis in `dist\`:
`MyLauncher_<version>_x64-setup.exe`, `mylauncher.exe`, `waypoint-server.exe`, `packtool.exe` –
jede Datei wird zum Schluss mit `signtool verify /pa` geprüft.

## Launcher-Update

Der Launcher aktualisiert sich selbst. Unter *Einstellungen → Launcher* steht die aktuelle Version,
daneben die Schaltfläche zum Aktualisieren. Geprüft wird beim Start und danach alle 15 Minuten
(abschaltbar). Ein gefundenes Update meldet sich einmalig als Hinweis und als Markierung in der Seitenleiste.

**Wie der Launcher ein Update annimmt** – alle vier Punkte müssen stimmen, sonst bricht er ab:

1. `update.json` nennt eine höhere Version als die laufende (`0.10.0` > `0.9.0`, Vorabversionen zählen als älter),
2. die heruntergeladene Datei hat die in `update.json` angegebene **SHA-256**-Prüfsumme,
3. die Datei trägt eine gültige **Authenticode-Signatur** (Kette bis zu einem Windows-Stammzertifikat),
4. der Signierer ist **unser** Zertifikat, verglichen über den Thumbprint aus `launcher.config.json`.

Erst dann startet der Installer und der Launcher beendet sich, damit seine Exe ersetzt werden kann.
Scheitert eine Prüfung, wird die Datei sofort gelöscht. Feed-URL und Thumbprint liest der Launcher
**nur aus der eingebauten Konfiguration** – eine Datei in `%APPDATA%` kann sie nicht ersetzen, sonst
ließe sich ein fremdes Update unterschieben. Zum Testen gegen einen anderen Feed gibt es
`MYLAUNCHER_UPDATE_FEED`.

**Veröffentlichen:**

```powershell
.\scripts\build-signed.ps1 -Notes "Was neu ist" -Publish
```

Das Skript baut und signiert, schreibt `dist\update.json` (Version aus `app/tauri.conf.json`, URL und
Prüfsumme des Installers) und legt mit `gh` das Release `v<Version>` an. Das Repository muss öffentlich
sein, sonst kann der Launcher die Dateien nicht laden. Ohne `-Publish` landet alles nur in `dist\`.

```json
{ "version": "0.3.0", "notes": "…", "pubDate": "2026-10-18",
  "url": "https://github.com/<repo>/releases/download/v0.3.0/MyLauncher_0.3.0_x64-setup.exe",
  "sha256": "…", "size": 2706640 }
```

## Xaero-Wegpunkte teilen

1. **Server starten** (z. B. auf deinem Server-Rechner, Linux oder Windows):
   ```bash
   cargo build --release -p waypoint-server
   waypoint-server --bind 0.0.0.0:8787 --data ./wegpunkte --token freunde:EIN-LANGES-GEHEIMNIS
   ```
   Mehrere Teams: `--token` mehrfach angeben. Fürs Internet einen Reverse-Proxy mit HTTPS davorschalten
   (z. B. Caddy: `wegpunkte.example.net { reverse_proxy 127.0.0.1:8787 }`).
2. **Im Launcher** unter *Einstellungen → Wegpunkt-Sync* die Server-URL und den Team-Schlüssel eintragen
   (oder die URL als `waypointServer` in `launcher.config.json` vorgeben).
3. Unter *Wegpunkte* die Welt (z. B. `gaming.stephanides-it.at`) mit **Teilen** freigeben.
4. Für jede Instanz **Automatisch über Prism** einschalten. Dafür muss Prism kurz geschlossen sein.
5. Im Spiel Wegpunkte dem Xaero-Set **„Team“** zuordnen. Nur diese werden geteilt,
   alle anderen bleiben privat. Mit „Alle Wegpunkte teilen“ gilt es für alle, Todespunkte bleiben trotzdem privat.

**Wie der Sync funktioniert:** Vor dem Spielstart holt Prism über den Hook den aktuellen Team-Stand. Nach dem Beenden
lädt es deine Änderungen hoch. Neu, geändert und gelöscht wird anhand des letzten Sync-Stands erkannt
(`mylauncher-sync.json` im Xaero-Weltordner). Während das Spiel läuft, wird nicht synchronisiert, weil Xaero seine
Dateien sonst wieder überschreibt. Ist der Server nicht erreichbar, startet das Spiel trotzdem normal. Das Log liegt in
`%APPDATA%\MyLauncher\waypoint-sync.log`.

## Konfiguration – `launcher.config.json`

Wird in die Exe eingebaut. Eine Datei gleichen Namens in `%APPDATA%\MyLauncher\` überschreibt einzelne Felder.

```json
{
  "brand":   { "name": "MyLauncher", "tagline": "Powered by Prism", "accent": "#7c5cff" },
  "newsUrl": "https://example.net/news.json",
  "news":    [ { "title": "…", "date": "2026-10-18", "body": "…", "image": "https://…", "url": "https://…" } ],
  "packs":   [ { "manifest": "https://example.net/pack/manifest.json" } ],
  "servers": [ { "name": "gaming.stephanides-it.at", "address": "gaming.stephanides-it.at", "instance": "26.2" } ],
  "hiddenInstances": [],
  "waypointServer": "https://wegpunkte.example.net",
  "update":  { "feed": "https://github.com/…/releases/latest/download/update.json",
               "signerThumbprint": "7C72A427…" }
}
```

`MYLAUNCHER_HOME` ersetzt `%APPDATA%\MyLauncher`, etwa für einen portablen Betrieb oder für Tests.
`update` wird als einziger Block **nicht** aus der Datei in `%APPDATA%` übernommen – siehe *Launcher-Update*.

## Modpack erstellen

```
mein-pack/
  files/       -> wird immer synchron gehalten (mods/, config/, resourcepacks/ …)
  defaults/    -> wird nur angelegt, wenn die Datei fehlt (z. B. options.txt)
```

```bash
cargo run -p launcher-core --bin packtool -- ./mein-pack --id mein-pack --name "Mein Pack" \
  --version 1.0.0 --mc 1.21.4 --loader fabric@0.16.10 --server play.example.net --memory 4096
```

Lade den Ordner hoch und trage die URL der `manifest.json` unter `packs` ein. Für ein Update erhöhst du `--version`.
Eigene Mods der Spieler bleiben erhalten, denn nur Dateien aus dem Manifest werden verwaltet.

### Abgleich

*Prüfen* bzw. *Aktualisieren* im Modpack-Tab verändert nichts, sondern zeigt zuerst eine Vorschau.
Dafür wird jede verwaltete Datei per SHA1 gegen das Manifest gehalten:

| | |
|---|---|
| **Neu** | kommt neu dazu |
| **Aktualisiert** | das Pack liefert eine andere Fassung als beim letzten Mal |
| **Wiederhergestellt** | lokal gelöscht, beschädigt oder von Hand geändert – das Pack selbst ist unverändert |
| **Entfernt** | nicht mehr im Pack |
| **Eigene Dateien** | in `mods/`, `resourcepacks/`, `shaderpacks/` gefunden, aber nicht aus dem Manifest – wird nie angefasst |

Erst der Knopf im Dialog schreibt. Modpacks werden außerdem alle 15 Minuten im Hintergrund auf neue
Versionen geprüft; nicht aktuelle Packs markiert die Seitenleiste.
