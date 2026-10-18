<#
.SYNOPSIS
  Baut MyLauncher (NSIS-Installer), waypoint-server und packtool als Release und signiert alles
  mit dem Certum-Code-Signing-Zertifikat (SimplySign).

.DESCRIPTION
  Voraussetzung: SimplySign Desktop läuft und ist angemeldet (Code aus der SimplySign-App).
  Ergebnis landet in dist\ – inklusive update.json für das Selbst-Update des Launchers.

  .\scripts\build-signed.ps1
  .\scripts\build-signed.ps1 -SkipTools                      # nur den Launcher-Installer
  .\scripts\build-signed.ps1 -Notes "Was neu ist" -Publish   # als GitHub-Release hochladen

.PARAMETER Notes
  Änderungstext, den der Launcher im Update-Hinweis anzeigt.

.PARAMETER Publish
  Legt mit `gh` ein Release v<Version> an und lädt Installer, update.json und die Tools hoch.
  Das Repository muss öffentlich sein, sonst kann der Launcher die Dateien nicht laden.
#>
param(
    [switch]$SkipTools,
    [string]$Notes = '',
    [switch]$Publish
)

$ErrorActionPreference = 'Stop'
$Thumbprint = '7C72A42764582F617CF3BD4F22231ED974B3EA61'
$TimestampUrl = 'http://time.certum.pl'
$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root 'dist'

# Version und Update-Ziel kommen aus den Dateien, die auch in die Exe eingebaut werden –
# so können update.json, Release-Tag und Launcher nicht auseinanderlaufen.
function Get-ReleaseInfo {
    $version = (Get-Content (Join-Path $Root 'app\tauri.conf.json') -Raw | ConvertFrom-Json).version
    $feed = (Get-Content (Join-Path $Root 'launcher.config.json') -Raw | ConvertFrom-Json).update.feed
    if (-not $feed) { throw 'launcher.config.json: update.feed ist leer – kein Update-Ziel bekannt.' }
    if ($feed -notmatch '^https://github\.com/([^/]+/[^/]+)/releases/') {
        throw "update.feed zeigt nicht auf ein GitHub-Release: $feed"
    }
    return [pscustomobject]@{ Version = $version; Repo = $Matches[1]; Feed = $feed }
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
$release = Get-ReleaseInfo
Write-Host "Version $($release.Version) für $($release.Repo)" -ForegroundColor Cyan
if ($Publish) {
    & gh auth status | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'gh ist nicht angemeldet – "gh auth login" ausführen.' }
}
Assert-SigningKey
New-Item -ItemType Directory -Force $Dist | Out-Null
$env:Path = "$(Split-Path $signTool);$env:Path"   # Tauri nutzt signtool aus dem PATH
Remove-Item Env:\ELECTRON_RUN_AS_NODE -ErrorAction SilentlyContinue

# 1) Launcher + Installer (Tauri signiert mylauncher.exe, den Installer und den Uninstaller)
Push-Location (Join-Path $Root 'app')
try {
    cargo tauri build --config tauri.signed.conf.json
    if ($LASTEXITCODE -ne 0) { throw 'cargo tauri build fehlgeschlagen' }
} finally { Pop-Location }

$installer = Get-ChildItem (Join-Path $Root 'target\release\bundle\nsis') -Filter '*-setup.exe' |
    Sort-Object LastWriteTime | Select-Object -Last 1

# Tauri signiert mylauncher.exe vor dem Bundling, stellt danach aber die unpatchte Datei wieder her –
# die Signatur ist dort also weg. Im Installer steckt die signierte Kopie; die freistehende Exe
# (fuer Entwicklung und den Hook-Modus) signieren wir hier noch einmal selbst.
$launcher = Join-Path $Root 'target\release\mylauncher.exe'
Invoke-Sign $signTool $launcher 'MyLauncher'
$outputs = @($installer.FullName, $launcher)

# 2) Zusatz-Tools
if (-not $SkipTools) {
    cargo build --release -p waypoint-server
    if ($LASTEXITCODE -ne 0) { throw 'waypoint-server Build fehlgeschlagen' }
    cargo build --release -p launcher-core --bin packtool
    if ($LASTEXITCODE -ne 0) { throw 'packtool Build fehlgeschlagen' }
    foreach ($tool in @(@('waypoint-server.exe', 'MyLauncher Waypoint Server'), @('packtool.exe', 'MyLauncher Packtool'))) {
        $file = Join-Path $Root "target\release\$($tool[0])"
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
$distInstaller = Get-Item (Join-Path $Dist $installer.Name)
$sha256 = (Get-FileHash $distInstaller.FullName -Algorithm SHA256).Hash.ToLower()
$feedFile = Join-Path $Dist 'update.json'
Write-Json $feedFile ([ordered]@{
    version = $release.Version
    notes   = $Notes
    pubDate = (Get-Date -Format 'yyyy-MM-dd')
    url     = "https://github.com/$($release.Repo)/releases/download/v$($release.Version)/$($installer.Name)"
    sha256  = $sha256
    size    = $distInstaller.Length
})
Write-Host "`nupdate.json: v$($release.Version), $([math]::Round($distInstaller.Length / 1MB, 2)) MB, SHA-256 $($sha256.Substring(0, 16))…"

# 5) Veröffentlichen
if ($Publish) {
    $assets = @($distInstaller.FullName, $feedFile)
    if (-not $SkipTools) {
        $assets += @((Join-Path $Dist 'waypoint-server.exe'), (Join-Path $Dist 'packtool.exe'))
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
