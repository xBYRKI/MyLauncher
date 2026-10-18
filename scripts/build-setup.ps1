<#
.SYNOPSIS
  Baut den Launcher als Release und packt ihn in einen **unsignierten** Installer – zum lokalen Testen.

.DESCRIPTION
  Für Veröffentlichungen build-signed.ps1 nehmen: Der Selbst-Update-Mechanismus akzeptiert
  nur Installer mit unserer Signatur. Ergebnis: dist\MyLauncher_<Version>_x64-setup-unsigned.exe

  .\scripts\build-setup.ps1
#>
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root 'dist'

$cargo = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
if ($cargo -notmatch '(?s)\[workspace\.package\].*?version\s*=\s*"([^"]+)"') { throw 'Version in Cargo.toml nicht gefunden.' }
$version = $Matches[1]

$makensis = @(
    (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'),
    'C:\Program Files (x86)\NSIS\makensis.exe',
    'C:\Program Files\NSIS\makensis.exe'
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $makensis) { $makensis = (Get-Command makensis.exe -ErrorAction SilentlyContinue).Source }
if (-not $makensis) { throw 'makensis.exe nicht gefunden – NSIS installieren (https://nsis.sourceforge.io).' }

Write-Host "Version $version" -ForegroundColor Cyan
cargo build --release -p mylauncher
if ($LASTEXITCODE -ne 0) { throw 'Launcher-Build fehlgeschlagen' }

New-Item -ItemType Directory -Force $Dist | Out-Null
$out = Join-Path $Dist "MyLauncher_${version}_x64-setup-unsigned.exe"
& $makensis -V2 "-DVERSION=$version" "-DSRC=$(Join-Path $Root 'target\release')" "-DOUTFILE=$out" (Join-Path $Root 'installer\mylauncher.nsi')
if ($LASTEXITCODE -ne 0) { throw 'makensis fehlgeschlagen' }

Write-Host "Fertig: $out ($([math]::Round((Get-Item $out).Length / 1MB, 1)) MB)" -ForegroundColor Green
