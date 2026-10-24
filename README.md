# Birkenfeld's Minecraft Launcher

Eigener Minecraft-Launcher in **Rust** mit nativer Oberfläche (egui) im Minecraft-Stil. **Prism Launcher läuft darunter als Engine:**
Prism übernimmt Microsoft-Login, Java, Loader-Installation und den Spielstart. Der Launcher liest Prisms Daten
direkt, steuert Prism über dessen Kommandozeile und klinkt sich für den Xaero-Wegpunkt-Sync als Prism-Hook ein.

```
mylauncher.exe (Rust, egui)
  ├─ liest     instances/*/instance.cfg, mmc-pack.json, accounts.json (nur Name/UUID)
  ├─ startet   prismlauncher --launch <Instanz> --profile <Konto> [--server <Adresse>]
  ├─ Modpacks  legt Prism-Instanzen an und hält Dateien per SHA1 aktuell
  ├─ Update    prüft update.json, lädt den Installer und akzeptiert nur unsere Signatur
  └─ Hooks     PreLaunchCommand / PostExitCommand = mylauncher --xaero-sync pre|post "$INST_MC_DIR"
                  └─ mylauncher-server (eigener Rust-Server, teilt Wegpunkte im Team)

mylauncher-server
  ├─ /admin/      Admin-Oberfläche: Modpacks, News, Server, Launcher-Updates
  ├─ /launcher/   Konfiguration, Manifeste, Dateien und update.json für den Launcher
  └─ /api/        Wegpunkt-Sync
```

## Aufbau

| Ordner | Inhalt |
|---|---|
| `app/` | Launcher-Oberfläche (egui/eframe, Minecraft-Optik) und Hook-Modus `--xaero-sync` |
| `app/assets/` | Pixel-Schrift [Monocraft](https://github.com/IdreesInc/Monocraft) (SIL OFL 1.1) und Logo |
| `installer/` | NSIS-Skript für den Installer |
| `crates/core/` | Prism-Brücke, Modpack-Sync, Server-Ping, Xaero-Parser, Wegpunkt-Sync; `packtool` |
| `crates/server/` | `mylauncher-server` – Admin-Oberfläche, Modpack-/Update-Auslieferung und Wegpunkt-Sync |
| `crates/protocol/` | Gemeinsame Typen von Launcher und Server |

## Bauen & Starten

```bash
cargo run -p mylauncher                  # Launcher (Entwicklung)
cargo build --release -p mylauncher      # target/release/mylauncher.exe
makensis /DVERSION=0.2.0 installer\mylauncher.nsi   # Installer (NSIS 3)
cargo test -p launcher-core              # Tests
```

In VS Code liegen dieselben Schritte als Tasks bereit (`.vscode/tasks.json`): **Strg+Umschalt+B** baut,
*Tasks: Run Task* bietet Starten, Release, Installer, signiertes Release/Veröffentlichen, Tests, Clippy,
lokalen Wegpunkt-Server und Integrationstest. **F5** startet den Launcher im Debugger.

```powershell
.\scripts\build-setup.ps1               # unsignierter Installer zum Testen → dist\
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
`MyLauncher_<version>_x64-setup.exe`, `mylauncher.exe`, `mylauncher-server.exe`, `packtool.exe` –
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

Das Skript baut und signiert, schreibt `dist\update.json` (Version aus `Cargo.toml` → `[workspace.package]`, URL und
Prüfsumme des Installers) und legt mit `gh` das Release `v<Version>` an. Das Repository muss öffentlich
sein, sonst kann der Launcher die Dateien nicht laden. Ohne `-Publish` landet alles nur in `dist\`.

```json
{ "version": "0.3.0", "notes": "…", "pubDate": "2026-10-18",
  "url": "https://github.com/<repo>/releases/download/v0.3.0/MyLauncher_0.3.0_x64-setup.exe",
  "sha256": "…", "size": 2706640 }
```

## Server & Admin-Oberfläche

`mylauncher-server` liefert Modpacks, News, Serverliste und Launcher-Updates aus und wird über eine
Weboberfläche gepflegt. Derselbe Prozess übernimmt den Wegpunkt-Sync.

```bash
cargo build --release -p mylauncher-server
mylauncher-server --bind 127.0.0.1:8787 --data ./daten   --admin-password "EIN-LANGES-ADMIN-PASSWORT"   --public-url https://mc.example.net   --token freunde:EIN-LANGES-GEHEIMNIS          # optional, für den Wegpunkt-Sync
```

Auch per Umgebung: `MYLAUNCHER_ADMIN_PASSWORD`, `MYLAUNCHER_PUBLIC_URL`, `WAYPOINT_BIND`, `WAYPOINT_DATA`,
`WAYPOINT_TOKENS`. Davor gehört ein Reverse-Proxy mit HTTPS (z. B. Caddy: `mc.example.net { reverse_proxy 127.0.0.1:8787 }`).
`--public-url` sollte gesetzt sein – daraus entstehen die absoluten Adressen in `update.json`.
Ohne die Angabe nimmt der Server `X-Forwarded-Host`/`Host` der Anfrage.

Die Oberfläche liegt unter **`https://mc.example.net/admin/`**:

| Bereich | |
|---|---|
| **Modpacks** | Pack anlegen (auch als Kopie), Dateien/Ordner per Drag & Drop hochladen, als *Vorgabedatei* markieren, umbenennen, entfernen. Änderungen landen in einem **Entwurf**; erst *Veröffentlichen* erzeugt eine neue Version. Ältere Versionen lassen sich wieder ausliefern (Rollback) oder in den Entwurf laden. |
| **News** | Einträge für die Startseite, Reihenfolge per Knopf |
| **Launcher** | Branding (Name, Untertitel, Akzentfarbe), Serverliste, Wegpunkt-Server |
| **Updates** | signierten Installer hochladen, Version anbieten oder zurückziehen |
| **System** | Adressen zum Einbauen in den Launcher, Speicherplatz, ungenutzte Dateien löschen |

Dateien liegen nach SHA1 abgelegt (`daten/launcher/blobs/`), jede Mod also nur einmal – auch über Packs und
Versionen hinweg. Die Wegpunkt-Daten bleiben wie bisher unter `daten/<team>/`; der Teamname `launcher` ist dafür reserviert.

**Launcher anbinden** – unter *System* stehen beide Adressen, die in `launcher.config.json` gehören:

```json
"remoteConfig": "https://mc.example.net/launcher/config.json",
"update": { "feed": "https://mc.example.net/launcher/update.json", "signerThumbprint": "7C72A427…" }
```

Der Launcher holt die Konfiguration beim Start und alle 15 Minuten und legt sie in
`%APPDATA%\MyLauncher\remote-config.json` ab, damit er auch offline startet. Packs, News, Server und Branding
vom Server ersetzen die eingebauten; die Datei in `%APPDATA%` hat weiterhin das letzte Wort.
`update` und `remoteConfig` nimmt er vom Server **nie** an – ein übernommener Server kann also keinen fremden
Update-Feed unterschieben, und der Installer muss ohnehin mit eurem Zertifikat signiert sein.

**API für Skripte** – alle Admin-Aufrufe gehen auch mit `Authorization: Bearer <Admin-Passwort>`, z. B. ein
frisch signiertes Release direkt hochladen:

```powershell
curl.exe -H "Authorization: Bearer $env:MYLAUNCHER_ADMIN_PASSWORD" -T dist\MyLauncher_0.3.0_x64-setup.exe `
  "https://mc.example.net/admin/api/releases?version=0.3.0&file=MyLauncher_0.3.0_x64-setup.exe&activate=true&notes=Neu"
```

## Eigener Server per SSH

Für den eigenen Linux-Server gibt es Skripte, die alles über SSH erledigen: Programm ausrollen, Releases und
Modpacks hochladen. Die Admin-API wird dabei über einen SSH-Tunnel angesprochen
(lokal `127.0.0.1:18787` → Server `127.0.0.1:8787`), die Admin-Oberfläche muss also nicht im Internet hängen.

**Einrichten** – einmalig:

```powershell
Copy-Item scripts\server.config.example.json scripts\server.config.json   # nicht eingecheckt
# host, user, publicUrl anpassen; SSH-Schlüssel auf dem Server hinterlegen (Login ohne Passwort)
.\scripts\deploy-server.ps1
```

| Feld | |
|---|---|
| `host`, `user`, `port`, `identityFile` | SSH-Zugang. `user` ist `root` oder darf `sudo` |
| `publicUrl` | Adresse, unter der Launcher den Server erreichen (ohne Pfad) |
| `adminPassword` | leer lassen – der erste Deploy erzeugt eins und trägt es hier ein |
| `waypointTokens` | `["freunde:GEHEIMNIS"]` für den Wegpunkt-Sync |
| `proxy` | `caddy`: Site-Block mit automatischem HTTPS wird eingerichtet; `none`: Proxy selbst bauen |
| `exposeAdmin` | `false` (Vorgabe): Caddy sperrt `/admin` nach außen, Zugang nur per Tunnel |
| `tunnelPort` | lokaler Port für den Tunnel |

`deploy-server.ps1` baut `mylauncher-server` für Linux (statisch gegen musl, gelinkt mit dem mitgelieferten
`rust-lld` – kein Docker, kein Cross-Compiler), kopiert ihn per `scp` und richtet ein:
`/opt/mylauncher/mylauncher-server`, `/etc/mylauncher/server.env` (nur root lesbar), Daten unter
`/var/lib/mylauncher`, systemd-Dienst `mylauncher-server` (eigener Benutzer, abgeschottet) und
`/etc/caddy/mylauncher.caddy`. Erneut ausführen = Update, die Daten bleiben.

| Skript | |
|---|---|
| `deploy-server.ps1` | Server bauen und installieren/aktualisieren |
| `server-admin.ps1` | Tunnel öffnen und Admin-Oberfläche im Browser starten |
| `server-logs.ps1` | Dienststatus und Log (`journalctl -f`) |
| `publish-pack.ps1` | Modpack-Ordner abgleichen und veröffentlichen (unten) |
| `build-signed.ps1 -Publish` | Launcher-Release hochladen und als Update anbieten (unten) |

Alle auch als VS-Code-Tasks unter *Eigener Server: …*.

**Modpacks hochladen** – der lokale Ordner ist die Quelle, der Server wird angeglichen:

```powershell
# neues Pack
.\scripts\publish-pack.ps1 -Pack survival -Dir D:\Packs\survival -Name "Survival" `
    -Minecraft 1.21.4 -Loader fabric@0.16.10 -Server gaming.stephanides-it.at -Memory 6144 -Listed
# Mods geändert
.\scripts\publish-pack.ps1 -Pack survival -Dir D:\Packs\survival -Notes "Sodium aktualisiert"
.\scripts\publish-pack.ps1 -Pack survival -Dir D:\Packs\survival -DryRun   # nur anzeigen
```

Aufbau wie beim packtool (`files\` synchron, `defaults\` nur anlegen). Ohne diese Unterordner gilt der ganze Ordner
als `files\` – mit `-Prefix mods` lässt sich auch nur ein Mods-Ordner hochladen. Übertragen werden nur neue und
geänderte Dateien (SHA1-Vergleich mit dem Entwurf), lokal fehlende fliegen raus. Danach wird veröffentlicht; ohne
`-Version` zählt das Skript die letzte Nummer hoch (`1.4.2` → `1.4.3`). Ohne Änderungen bleibt die Version.
`-NoPublish` füllt nur den Entwurf, z. B. um in der Oberfläche noch etwas anzupassen.

**Launcher-Releases** – `build-signed.ps1 -Publish` lädt dorthin, wohin `update.feed` zeigt. Steht dort die Adresse
des eigenen Servers, geht der signierte Installer per Tunnel hoch, wird sofort als Update angeboten und die
Prüfsumme auf dem Server gegengeprüft; `update.json` erzeugt der Server selbst.

**Umzug von GitHub:** Bereits installierte Launcher kennen nur den eingebauten GitHub-Feed. Daher in
`launcher.config.json` `remoteConfig` und `update.feed` auf den eigenen Server stellen, Version erhöhen und einmal
mit `build-signed.ps1 -Notes "…" -Publish -GitHubRepo xBYRKI/MyLauncher` veröffentlichen. Die Version geht auf den
eigenen Server **und** als GitHub-Release raus; alte Launcher finden sie noch auf GitHub, und ab dieser Version
kommen alle Updates vom eigenen Server.

## Xaero-Wegpunkte teilen

1. **Server starten** (z. B. auf deinem Server-Rechner, Linux oder Windows):
   ```bash
   cargo build --release -p mylauncher-server
   mylauncher-server --bind 0.0.0.0:8787 --data ./daten --token freunde:EIN-LANGES-GEHEIMNIS
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
  "brand":   { "name": "Birkenfeld's Minecraft Launcher", "tagline": "Powered by Prism", "accent": "#7c5cff" },
  "newsUrl": "https://example.net/news.json",
  "news":    [ { "title": "…", "date": "2026-10-18", "body": "…", "image": "https://…", "url": "https://…" } ],
  "packs":   [ { "manifest": "https://example.net/pack/manifest.json" } ],
  "servers": [ { "name": "gaming.stephanides-it.at", "address": "gaming.stephanides-it.at", "instance": "26.2" } ],
  "hiddenInstances": [],
  "waypointServer": "https://wegpunkte.example.net",
  "remoteConfig": "https://mc.example.net/launcher/config.json",
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

Einfacher geht es über die Admin-Oberfläche (siehe oben). Ohne Server: Lade den Ordner hoch und trage die URL der `manifest.json` unter `packs` ein. Für ein Update erhöhst du `--version`.
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
