# MyLauncher

Eigener Minecraft-Launcher in **Rust** (Tauri 2). **Prism Launcher läuft darunter als Engine:**
Prism übernimmt Microsoft-Login, Java, Loader-Installation und den Spielstart. MyLauncher liest Prisms Daten
direkt, steuert Prism über dessen Kommandozeile und klinkt sich für den Xaero-Wegpunkt-Sync als Prism-Hook ein.

```
MyLauncher (Tauri, Rust)
  ├─ liest     instances/*/instance.cfg, mmc-pack.json, accounts.json (nur Name/UUID)
  ├─ startet   prismlauncher --launch <Instanz> --profile <Konto> [--server <Adresse>]
  ├─ Modpacks  legt Prism-Instanzen an und hält Dateien per SHA1 aktuell
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
3. Unter *Wegpunkte* die Welt (z. B. `gaming.byrki.dev`) mit **Teilen** freigeben.
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
  "servers": [ { "name": "gaming.byrki.dev", "address": "gaming.byrki.dev", "instance": "26.2" } ],
  "hiddenInstances": [],
  "waypointServer": "https://wegpunkte.example.net"
}
```

`MYLAUNCHER_HOME` ersetzt `%APPDATA%\MyLauncher`, etwa für einen portablen Betrieb oder für Tests.

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
