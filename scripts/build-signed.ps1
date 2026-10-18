<#
.SYNOPSIS
  Baut MyLauncher (NSIS-Installer), waypoint-server und packtool als Release und signiert alles
  mit dem Certum-Code-Signing-Zertifikat (SimplySign).

.DESCRIPTION
  Voraussetzung: SimplySign Desktop läuft und ist angemeldet (Code aus der SimplySign-App).
  Ergebnis landet in dist\.

  .\scripts\build-signed.ps1
  .\scripts\build-signed.ps1 -SkipTools   # nur den Launcher-Installer
#>
param(
    [switch]$SkipTools
)

$ErrorActionPreference = 'Stop'
$Thumbprint = '7C72A42764582F617CF3BD4F22231ED974B3EA61'
$TimestampUrl = 'http://time.certum.pl'
$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root 'dist'

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
    Write-Host ("  {0,-45} {1}  {2}" -f (Split-Path $File -Leaf), $sig.Status, $sig.SignerCertificate.Subject.Split(',')[0])
}

$signTool = Find-SignTool
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
$outputs = @($installer.FullName, (Join-Path $Root 'target\release\mylauncher.exe'))

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
Write-Host "`nFertig: $Dist" -ForegroundColor Green
