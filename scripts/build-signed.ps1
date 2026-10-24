<#
.SYNOPSIS
  Baut MyLauncher (NSIS-Installer), mylauncher-server und packtool als Release und signiert alles
  mit dem Certum-Code-Signing-Zertifikat (SimplySign).

.DESCRIPTION
  Voraussetzung: SimplySign Desktop läuft und ist angemeldet (Code aus der SimplySign-App).
  Ergebnis landet in dist\ – inklusive update.json für das Selbst-Update des Launchers.

  .\scripts\build-signed.ps1
  .\scripts\build-signed.ps1 -SkipTools                      # nur den Launcher-Installer
  .\scripts\build-signed.ps1 -Notes "Was neu ist" -Publish   # dorthin, wohin update.feed zeigt
  .\scripts\build-signed.ps1 -Notes "…" -Publish -Server     # zusätzlich auf den eigenen Server

.PARAMETER Notes
  Änderungstext, den der Launcher im Update-Hinweis anzeigt.

.PARAMETER Publish
  Zeigt update.feed in launcher.config.json auf ein GitHub-Release, legt `gh` ein Release v<Version>
  an und lädt Installer, update.json und die Tools hoch (Repository muss öffentlich sein).
  Zeigt er auf den eigenen Server (publicUrl aus scripts\server.config.json), wird der Installer
  per SSH-Tunnel hochgeladen und sofort als Update angeboten – update.json erzeugt dann der Server.

.PARAMETER Server
  Mit -Publish: den Installer zusätzlich auf den eigenen Server laden, auch wenn update.feed
  noch auf GitHub zeigt.

.PARAMETER GitHubRepo
  Mit -Publish: zusätzlich ein GitHub-Release in diesem Repository anlegen (z. B. xBYRKI/MyLauncher),
  auch wenn update.feed schon auf den eigenen Server zeigt. Für den Umzug: Ältere Launcher holen
  diese Version noch von GitHub und finden darin den neuen Feed.
#>
param(
    [switch]$SkipTools,
    [string]$Notes = '',
    [switch]$Publish,
    [switch]$Server,
    [string]$GitHubRepo = ''
)

$ErrorActionPreference = 'Stop'
$Thumbprint = '7C72A42764582F617CF3BD4F22231ED974B3EA61'
$TimestampUrl = 'http://time.certum.pl'
$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root 'dist'
# Eigener Build-Ordner: ein laufender oder vom Virenscanner gesperrter target\release\mylauncher.exe
# blockiert sonst den Linker (LNK1104).
$env:CARGO_TARGET_DIR = Join-Path $Root 'target\publish'
$Out = Join-Path $env:CARGO_TARGET_DIR 'release'

# Version und Update-Ziel kommen aus den Dateien, die auch in die Exe eingebaut werden –
# so können update.json, Release-Tag und Launcher nicht auseinanderlaufen.
function Get-ReleaseInfo {
    $cargo = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
    if ($cargo -notmatch '(?s)\[workspace\.package\].*?version\s*=\s*"([^"]+)"') { throw 'Version in Cargo.toml nicht gefunden.' }
    $version = $Matches[1]
    $feed = (Get-Content (Join-Path $Root 'launcher.config.json') -Raw | ConvertFrom-Json).update.feed
    if (-not $feed) { throw 'launcher.config.json: update.feed ist leer – kein Update-Ziel bekannt.' }
    $repo = if ($feed -match '^https://github\.com/([^/]+/[^/]+)/releases/') { $Matches[1] } else { $null }
    return [pscustomobject]@{ Version = $version; Repo = $repo; Feed = $feed }
}

# Ohne BOM schreiben: serde_json (und die meisten JSON-Parser) stolpern darüber.
function Write-Json([string]$Path, $Value) {
    $json = $Value | ConvertTo-Json -Depth 5
    [System.IO.File]::WriteAllText($Path, $json, (New-Object System.Text.UTF8Encoding $false))
}

function Find-SignTool {
    $kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $tool = Get-ChildItem $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object FullName -like '*\x64\*' | Sort-Object FullName | Select-Object -Last 1
    if (-not $tool) { throw 'signtool.exe nicht gefunden – Windows SDK installieren.' }
    return $tool.FullName
}

function Find-Makensis {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'),
        'C:\Program Files (x86)\NSIS\makensis.exe',
        'C:\Program Files\NSIS\makensis.exe'
    )
    $found = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $found) { $found = (Get-Command makensis.exe -ErrorAction SilentlyContinue).Source }
    if (-not $found) { throw 'makensis.exe nicht gefunden – NSIS installieren (https://nsis.sourceforge.io).' }
    return $found
}

function Assert-SigningKey {
    $cert = Get-Item "Cert:\CurrentUser\My\$Thumbprint" -ErrorAction SilentlyContinue
    if (-not $cert) {
        throw "Zertifikat $Thumbprint nicht gefunden – SimplySign Desktop starten und anmelden (es blendet das Zertifikat nur bei aktiver Verbindung ein)."
    }
    if ($cert.NotAfter -lt (Get-Date)) { throw "Zertifikat ist am $($cert.NotAfter) abgelaufen." }
    try {
        # Probe-Signatur: schlägt fehl, wenn SimplySign nicht angemeldet ist.
        $key = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($cert)
        $null = $key.SignData([byte[]](1, 2, 3), [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
    } catch {
        throw "Privater Schlüssel nicht erreichbar – ist SimplySign Desktop gestartet und angemeldet?`n$($_.Exception.Message)"
    }
    Write-Host "Zertifikat: $($cert.Subject) (gültig bis $($cert.NotAfter.ToString('dd.MM.yyyy')))" -ForegroundColor Green
}

function Invoke-Sign([string]$SignTool, [string]$File, [string]$Description) {
    & $SignTool sign /sha1 $Thumbprint /fd sha256 /tr $TimestampUrl /td sha256 /d $Description $File
    if ($LASTEXITCODE -ne 0) { throw "Signieren fehlgeschlagen: $File" }
}

function Test-Signature([string]$SignTool, [string]$File) {
    & $SignTool verify /pa /q $File
    if ($LASTEXITCODE -ne 0) { throw "Signatur ungültig: $File" }
    $sig = Get-AuthenticodeSignature $File
    if (-not $sig.TimeStamperCertificate) { throw "Signatur ohne Zeitstempel: $File" }
    Write-Host ("  {0,-45} {1}  {2}  Zeitstempel: {3}" -f (Split-Path $File -Leaf), $sig.Status,
        $sig.SignerCertificate.Subject.Split(',')[0], $sig.TimeStamperCertificate.Subject.Split(',')[0])
}

$signTool = Find-SignTool
$makensis = Find-Makensis
$release = Get-ReleaseInfo
$feedIsGitHub = [bool]$release.Repo
if ($GitHubRepo) { $release.Repo = $GitHubRepo }
$toGitHub = $Publish -and $release.Repo
$toServer = $Publish -and ($Server -or -not $feedIsGitHub)
$targets = @()
if ($toServer) {
    # Vor dem langen Build prüfen, ob der eigene Server erreichbar ist und der Feed dazu passt.
    Import-Module (Join-Path $PSScriptRoot 'lib\MyLauncherServer.psm1') -Force
    $serverCfg = Get-ServerConfig
    if (-not $feedIsGitHub -and $release.Feed -ne "$($serverCfg.publicUrl)/launcher/update.json") {
        throw "update.feed ($($release.Feed)) ist weder GitHub noch $($serverCfg.publicUrl)/launcher/update.json."
    }
    Test-SshLogin $serverCfg
    $targets += $serverCfg.publicUrl
}
if ($toGitHub) { $targets += "GitHub $($release.Repo)" }
Write-Host "Version $($release.Version)$(if ($targets) { ' → ' + ($targets -join ' + ') })" -ForegroundColor Cyan
if ($toGitHub) {
    & gh auth status | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'gh ist nicht angemeldet – "gh auth login" ausführen.' }
}
Assert-SigningKey
New-Item -ItemType Directory -Force $Dist | Out-Null
$env:Path = "$(Split-Path $signTool);$env:Path"   # makensis signiert Installer und Uninstaller über signtool im PATH

# 1) Launcher bauen und signieren, dann in den Installer packen
cargo build --release -p mylauncher
if ($LASTEXITCODE -ne 0) { throw 'Launcher-Build fehlgeschlagen' }
$launcher = Join-Path $Out 'mylauncher.exe'
Invoke-Sign $signTool $launcher "Birkenfeld's Minecraft Launcher"

$installerPath = Join-Path $Out "MyLauncher_$($release.Version)_x64-setup.exe"
& $makensis -V2 "-DVERSION=$($release.Version)" "-DSRC=$(Split-Path $launcher)" "-DOUTFILE=$installerPath" `
    "-DTHUMBPRINT=$Thumbprint" (Join-Path $Root 'installer\mylauncher.nsi')
if ($LASTEXITCODE -ne 0) { throw 'makensis fehlgeschlagen' }
$installer = Get-Item $installerPath
$outputs = @($installer.FullName, $launcher)

# 2) Zusatz-Tools
if (-not $SkipTools) {
    cargo build --release -p mylauncher-server
    if ($LASTEXITCODE -ne 0) { throw 'mylauncher-server Build fehlgeschlagen' }
    cargo build --release -p launcher-core --bin packtool
    if ($LASTEXITCODE -ne 0) { throw 'packtool Build fehlgeschlagen' }
    foreach ($tool in @(@('mylauncher-server.exe', 'MyLauncher Server'), @('packtool.exe', 'MyLauncher Packtool'))) {
        $file = Join-Path $Out $tool[0]
        Invoke-Sign $signTool $file $tool[1]
        $outputs += $file
    }
}

# 3) Prüfen und nach dist\ kopieren
Write-Host "`nSignaturen:" -ForegroundColor Cyan
foreach ($file in $outputs) {
    Test-Signature $signTool $file
    Copy-Item $file $Dist -Force
}

# 4) update.json – daraus erfährt der Launcher von der neuen Version. Der Hash ist das
#    erste von zwei Schlössern; das zweite ist die Signatur des Installers selbst.
#    Der eigene Server braucht die Datei nicht, er erzeugt update.json selbst.
$distInstaller = Get-Item (Join-Path $Dist $installer.Name)
$sha256 = (Get-FileHash $distInstaller.FullName -Algorithm SHA256).Hash.ToLower()
$feedFile = Join-Path $Dist 'update.json'
if ($release.Repo) {
    Write-Json $feedFile ([ordered]@{
        version = $release.Version
        notes   = $Notes
        pubDate = (Get-Date -Format 'yyyy-MM-dd')
        url     = "https://github.com/$($release.Repo)/releases/download/v$($release.Version)/$($installer.Name)"
        sha256  = $sha256
        size    = $distInstaller.Length
    })
}
Write-Host "`nInstaller: v$($release.Version), $([math]::Round($distInstaller.Length / 1MB, 2)) MB, SHA-256 $($sha256.Substring(0, 16))…"

# 5) Veröffentlichen
if ($toServer) {
    Write-Host "`nLade auf $($serverCfg.publicUrl) hoch (SSH-Tunnel) …" -ForegroundColor Cyan
    $tunnel = Open-AdminTunnel $serverCfg
    try {
        $q = ConvertTo-Query @{ version = $release.Version; file = $installer.Name; notes = $Notes; activate = 'true' }
        $result = Invoke-Admin $tunnel PUT "/releases?$q" -InFile $distInstaller.FullName
        $uploaded = $result.list | Where-Object version -eq $release.Version
        if ($uploaded.sha256 -ne $sha256) { throw "Prüfsumme auf dem Server weicht ab: $($uploaded.sha256)" }
    } finally {
        Close-AdminTunnel $tunnel
    }
    Write-Host "Release v$($release.Version) auf $($serverCfg.publicUrl) als Update angeboten." -ForegroundColor Green
}
if ($toGitHub) {
    $assets = @($distInstaller.FullName, $feedFile)
    if (-not $SkipTools) {
        $assets += @((Join-Path $Dist 'mylauncher-server.exe'), (Join-Path $Dist 'packtool.exe'))
    }
    $ghArgs = @('release', 'create', "v$($release.Version)",
        '--repo', $release.Repo, '--title', $release.Version)
    if ($Notes) { $ghArgs += @('--notes', $Notes) } else { $ghArgs += '--generate-notes' }
    & gh @ghArgs @assets
    if ($LASTEXITCODE -ne 0) { throw 'gh release create fehlgeschlagen' }
    Write-Host "Release v$($release.Version) veröffentlicht." -ForegroundColor Green
}

Write-Host "`nFertig: $Dist" -ForegroundColor Green
if (-not $Publish) {
    Write-Host "Zum Veröffentlichen: .\scripts\build-signed.ps1 -Notes `"…`" -Publish" -ForegroundColor DarkGray
}
