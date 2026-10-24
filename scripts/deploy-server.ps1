<#
.SYNOPSIS
  Baut mylauncher-server für Linux und installiert/aktualisiert ihn per SSH auf dem eigenen Server.

.DESCRIPTION
  Einstellungen stehen in scripts\server.config.json (Vorlage: server.config.example.json).
  Beim ersten Lauf wird ein Admin-Passwort erzeugt und dort gespeichert.

  Auf dem Server entsteht:
    /opt/mylauncher/mylauncher-server     Programm
    /etc/mylauncher/server.env            Umgebung (nur root lesbar)
    /var/lib/mylauncher/                  Daten: Modpacks, Releases, Wegpunkte
    mylauncher-server.service             systemd-Dienst, lauscht auf 127.0.0.1:8787
    /etc/caddy/mylauncher.caddy           HTTPS über Caddy (bei "proxy": "caddy")

  Mit "exposeAdmin": false sperrt Caddy /admin nach außen – die Oberfläche öffnet dann
  .\scripts\server-admin.ps1 über einen SSH-Tunnel.

  .\scripts\deploy-server.ps1
  .\scripts\deploy-server.ps1 -SkipBuild    # vorhandenes Linux-Binary erneut ausrollen

  Der SSH-Benutzer muss root sein oder sudo dürfen (dann wird ggf. nach dem sudo-Passwort gefragt).
#>
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\MyLauncherServer.psm1') -Force
$Root = Get-RepoRoot
$Target = 'x86_64-unknown-linux-musl'
$TargetDir = Join-Path $Root 'target\linux'
$Binary = Join-Path $TargetDir "$Target\release\mylauncher-server"

$cfg = Get-ServerConfig
if (-not $cfg.adminPassword) {
    $cfg.adminPassword = New-Secret 32
    Save-ServerConfig $cfg
    Write-Host "Admin-Passwort erzeugt und in scripts\server.config.json gespeichert." -ForegroundColor Yellow
}
foreach ($t in $cfg.waypointTokens) {
    if ($t -notmatch '^[A-Za-z0-9_-]+:\S{8,}$') { throw "waypointTokens: '$t' – Format team:GEHEIMNIS (mind. 8 Zeichen)" }
}
if ($cfg.proxy -notin 'caddy', 'none') { throw 'server.config.json: proxy muss "caddy" oder "none" sein.' }

Write-Host "Ziel: $(Get-SshTarget $cfg) → $($cfg.publicUrl)" -ForegroundColor Cyan
Test-SshLogin $cfg

# 1) Linux-Binary – statisch gegen musl gelinkt, mit dem in Rust mitgelieferten rust-lld.
#    Kein Docker, kein Cross-Linker nötig; der Server hat keine C-Abhängigkeiten.
if (-not $SkipBuild) {
    if (-not (rustup target list --installed | Select-String -SimpleMatch $Target)) {
        rustup target add $Target
        if ($LASTEXITCODE -ne 0) { throw "rustup target add $Target fehlgeschlagen" }
    }
    $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = 'rust-lld'
    $env:CARGO_TARGET_DIR = $TargetDir
    cargo build --release -p mylauncher-server --target $Target
    if ($LASTEXITCODE -ne 0) { throw 'Build für Linux fehlgeschlagen' }
}
if (-not (Test-Path $Binary)) { throw "Kein Linux-Binary unter $Binary – ohne -SkipBuild ausführen." }

# 2) Dateien zusammenstellen (LF-Zeilenenden, sonst stolpert sh/systemd)
$stage = Join-Path ([System.IO.Path]::GetTempPath()) "mylauncher-deploy-$([guid]::NewGuid().ToString('n'))"
New-Item -ItemType Directory $stage | Out-Null
try {
    Copy-Item $Binary (Join-Path $stage 'mylauncher-server')
    foreach ($f in 'install.sh', 'mylauncher-server.service') {
        Write-Utf8 (Join-Path $stage $f) (Get-Content (Join-Path $Root "deploy\$f") -Raw -Encoding UTF8) -Lf
    }
    # systemd liest EnvironmentFile ohne Shell: Werte nicht quoten, keine Leerzeichen im Passwort.
    $envLines = @(
        "WAYPOINT_BIND=127.0.0.1:8787",
        "WAYPOINT_DATA=/var/lib/mylauncher",
        "MYLAUNCHER_ADMIN_PASSWORD=$($cfg.adminPassword)",
        "MYLAUNCHER_PUBLIC_URL=$($cfg.publicUrl)"
    )
    if (@($cfg.waypointTokens).Count) { $envLines += "WAYPOINT_TOKENS=$(@($cfg.waypointTokens) -join ',')" }
    Write-Utf8 (Join-Path $stage 'server.env') (($envLines -join "`n") + "`n")

    if ($cfg.proxy -eq 'caddy') {
        $siteHost = ([uri]$cfg.publicUrl).Authority
        $site = if ($cfg.publicUrl.StartsWith('http://')) { "http://$siteHost" } else { $siteHost }
        $adminBlock = if ($cfg.exposeAdmin) { '' } else {
            "`t# Admin nur über SSH-Tunnel (scripts/server-admin.ps1)`n`t@admin path / /admin /admin/*`n`trespond @admin 404`n"
        }
        Write-Utf8 (Join-Path $stage 'mylauncher.caddy') "# Von scripts/deploy-server.ps1 erzeugt – Änderungen werden beim nächsten Deploy überschrieben.`n$site {`n$adminBlock`treverse_proxy 127.0.0.1:8787`n}`n"
    }

    # 3) Hochladen und installieren
    $remoteDir = "/tmp/mylauncher-deploy-$([guid]::NewGuid().ToString('n').Substring(0, 8))"
    Write-Host "Lade hoch nach $remoteDir …" -ForegroundColor Cyan
    Invoke-Ssh $cfg "mkdir -m 700 $remoteDir"
    Copy-ToServer $cfg (Get-ChildItem $stage | ForEach-Object FullName) $remoteDir
    $sudo = if ($cfg.user -eq 'root') { '' } else { 'sudo ' }
    Invoke-Ssh $cfg "${sudo}sh $remoteDir/install.sh" -Tty:($cfg.user -ne 'root')
} finally {
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
}

# 4) Über den Tunnel prüfen, ob die Admin-API antwortet
$tunnel = Open-AdminTunnel $cfg
try {
    $status = Invoke-Admin $tunnel GET '/status'
} finally {
    Close-AdminTunnel $tunnel
}
Write-Host "`nServer läuft." -ForegroundColor Green
$status | ConvertTo-Json -Depth 4 | Write-Host
Write-Host @"

In launcher.config.json eintragen (dann Launcher neu bauen und einmal ausliefern):
  "remoteConfig": "$($cfg.publicUrl)/launcher/config.json",
  "update": { "feed": "$($cfg.publicUrl)/launcher/update.json", ... }

Admin-Oberfläche:  .\scripts\server-admin.ps1
Modpack hochladen: .\scripts\publish-pack.ps1 -Pack <id> -Dir <Ordner>
Release:           .\scripts\build-signed.ps1 -Notes "…" -Publish
"@
